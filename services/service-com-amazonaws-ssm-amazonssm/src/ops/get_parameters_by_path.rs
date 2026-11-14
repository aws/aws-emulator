// SPDX-License-Identifier: CC0-1.0

//! @op GetParametersByPath
//! Retrieve the parameters stored under a level of a parameter hierarchy — the
//! hierarchy read of Parameter Store. This is the ONLY file to touch for this operation.
//!
//! The answer is computed from the parameters `PutParameter` persisted (see
//! [`crate::state`]): this Region's parameters whose name carries the requested `Path`
//! as a whole prefix of its levels, in ascending name order — the level immediately
//! below the path by default, every level under it when `Recursive` is set. Each one is
//! reported at its newest version, or at the version a `Label` filter names. Nothing
//! here mutates the store — a read must not change what a later caller sees.
//!
//! ## AWS's own hierarchy
//!
//! A path inside the hierarchy AWS publishes its own parameters in reads THAT catalog
//! ([`crate::state::public_parameters`]) instead: no parameter of this account can live
//! there, and AWS publishes none in the account's own hierarchy, so each path is answered
//! from the one source that can hold anything under it. Both sources are read in the same
//! ascending name order, so filtering, paging and the resume cursor are the same for
//! either.
//!
//! That hierarchy is NAMESPACES rather than an empty tree a caller may walk: a path in it
//! that this endpoint serves no name at or under is refused with `AccessDeniedException`
//! ([`crate::state::public_parameters::invalid_namespace_message`]) rather than answered
//! with an empty page, which a caller would read as a namespace that exists and holds
//! nothing. Nothing of the account is hidden by that refusal — no parameter of the account
//! can be in that hierarchy at all.
//!
//! One hierarchy of that AWS-owned namespace is the reserved path a single read serves an
//! Amazon Web Services Secrets Manager secret through
//! ([`crate::state::secretsmanager_reference`]). A hierarchy read never enumerates it: what
//! lies under it are SECRETS, which the published catalog holds none of and this account's
//! store cannot hold at all, so a listing of that path — or of any level above it — reports
//! nothing for them. A secret is reached by naming its reference, not by walking a hierarchy
//! the account owns none of.
//!
//! ## Filters
//!
//! `ParameterFilters` accepts the three keys this operation publishes — `Type`, `KeyId`
//! and `Label` ([`crate::state::PARAMETERS_BY_PATH_FILTER_KEYS`]) — and refuses every
//! other key the modelled pattern allows with `InvalidFilterKey`, which is what the
//! model's `Key` doc excludes here. An option outside `Equals` / `BeginsWith` (and
//! `Equals` alone for `Label`) is `InvalidFilterOption`, and a filter carrying no value,
//! or a `Type` equality no `ParameterType` can satisfy, is `InvalidFilterValue`.
//!
//! ## `WithDecryption` and `SecureString`
//!
//! As for `GetParameter`: a `SecureString`'s value is stored as the KMS ciphertext its put
//! sealed, so `WithDecryption` selects which form each reported value takes — the stored
//! ciphertext, base64-encoded, when it is absent or false, and the plaintext, opened under
//! that parameter's own key, when it is true. Every value this read reports has to be
//! opened before the answer is served, so a key that refuses one of them refuses the whole
//! read with `AccessDeniedException`, and a value no key of this endpoint opens any more is
//! `InvalidKeyId`.
//!
//! ## Paging
//!
//! The token this operation hands out carries the `Name` of the parameter the next page
//! starts at, so a resume is a position in the store's own total order rather than an
//! offset into a list that may have changed between two pages. It is opaque, and carries
//! that position behind an authenticator only this store can produce
//! ([`crate::state::parameter_pagination`]): a token this read did not ISSUE names no
//! position, and is the modelled `InvalidNextToken` rather than a page the caller was
//! never offered.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `GetParametersByPath::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `GetParametersByPath`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json")
//! ```
//!
//! Retrieve information about one or more parameters under a specified level in a
//! hierarchy. Request results are returned on a best-effort basis. If you specify
//! MaxResults in the request, the response includes information up to the limit specified.
//! The number of items returned, however, can be between zero and the value of MaxResults.
//! If the service reaches an internal limit while processing the results, it stops the
//! operation and returns the matching values up to that point and a NextToken. You can
//! specify the NextToken in a subsequent call to get the next set of results. Parameter
//! names can't contain spaces. The service removes any spaces specified for the beginning
//! or end of a parameter name. If the specified name for a parameter contains spaces
//! between characters, the request fails with a ValidationException error.
//!
//! ## Input
//!
//! * `MaxResults` (GetParametersByPathMaxResults, range 1..10) — The maximum number of items
//!   to return for this call. The call also returns a token that you can specify in a
//!   subsequent call to get the next set of results.
//! * `NextToken` (NextToken) — A token to start the list. Use this token to get the next set
//!   of results.
//! * `ParameterFilters` (ParameterStringFilterList) — Filters to limit the request results.
//!   The following Key values are supported for GetParametersByPath: Type, KeyId, and Label.
//!   The following Key values aren't supported for GetParametersByPath: tag, DataType, Name,
//!   Path, and Tier.
//! * `Path` (PSParameterName, len 1..2048) **required** — The hierarchy for the parameter.
//!   Hierarchies start with a forward slash (/). The hierarchy is the parameter name except
//!   the last part of the parameter. For the API call to succeed, the last part of the
//!   parameter name can't be in the path. A parameter name hierarchy can have a maximum of 15
//!   levels. Here is an example of a hierarchy: /Finance/Prod/IAD/WinServ2016/license33
//! * `Recursive` (Boolean, default false) — Retrieve all parameters within a hierarchy. If a
//!   user has access to a path, then the user can access all levels of that path. For
//!   example, if a user has permission to access path /a, then the user can also access /a/b.
//!   Even if a user has explicitly been denied access in IAM for parameter /a/b, they can
//!   still call the GetParametersByPath API operation recursively for /a and view /a/b.
//! * `WithDecryption` (Boolean, default false) — Retrieve all parameters in a hierarchy with
//!   their value decrypted.
//!
//! ## Output
//!
//! * `NextToken` (NextToken) — The token for the next set of items to return. Use this token
//!   to get the next set of results.
//! * `Parameters` (ParameterList) — A list of parameters found in the specified hierarchy.
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `InvalidFilterKey` (HTTP 400) — The specified key isn't valid.
//! * `InvalidFilterOption` (HTTP 400) — The specified filter option isn't valid. Valid
//!   options are Equals and BeginsWith. For Path filter, valid options are Recursive and
//!   OneLevel.
//! * `InvalidFilterValue` (HTTP 400) — The filter value isn't valid. Verify the value and try
//!   again.
//! * `InvalidKeyId` (HTTP 400) — The query key ID isn't valid.
//! * `InvalidNextToken` (HTTP 400) — The specified token isn't valid.
//! * `AccessDeniedException` (HTTP 400) — widened in through `extra_errors` (see below):
//!   "The requester doesn't have permissions to perform the requested operation", which is
//!   what a `WithDecryption` read of a parameter whose KMS key does not permit the caller
//!   `kms:Decrypt` is refused with (see [`crate::state::secure_string`]).

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetParametersByPathError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssm-amazonssm.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetParametersByPathError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `AccessDeniedException` is widened in through `extra_errors`: this SERVICE models it
// ("The requester doesn't have permissions to perform the requested operation") and other
// operations declare it, and it is the refusal a decrypt the parameter's KMS key does not
// permit this caller answers with.
#[smithy_macros::operation_error(
    "com.amazonaws.ssm#GetParametersByPath",
    extra_errors = ["AccessDeniedException"]
)]
#[derive(Debug)]
pub enum GetParametersByPathErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerError(InternalServerError),
    InvalidFilterKey(InvalidFilterKey),
    InvalidFilterOption(InvalidFilterOption),
    InvalidFilterValue(InvalidFilterValue),
    InvalidKeyId(InvalidKeyId),
    InvalidNextToken(InvalidNextToken),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_parameters_by_path::GetParametersByPathError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetParametersByPathError = crate::scaffolded::Scaffolded<GetParametersByPathErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetParametersByPathErrorModeled> for GetParametersByPathError {
    fn from(modeled: GetParametersByPathErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetParametersByPath` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetParametersByPath<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetParametersByPath<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetParametersByPath<Eng>
{
    type Input = GetParametersByPathInput;
    type Output = GetParametersByPathOutput;
    type Error = GetParametersByPathError;

    const NAME: &'static str = "GetParametersByPath";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PARAMETERS_BY_PATH_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PARAMETERS_BY_PATH_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state::parameter_pagination::{page_cursor, page_token, PagedRead};
        use crate::state::{
            canonical_name, invalid_filter_key_message, invalid_filter_option_message,
            invalid_next_token_message, is_parameters_by_path_filter_key, is_under_path,
            parameter_path, parameters_by_path_filter_options, parameters_by_path_page_size,
            request_region, FILTER_KEY_TYPE, FILTER_OPTION_EQUALS, PARAMETERS_BY_PATH_FILTER_KEYS,
        };

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetParametersByPathErrorModeled::InternalServerError(
                InternalServerError {
                    message: Some(
                        "this service is mounted on an engine that holds no state".to_string(),
                    ),
                },
            )
            .into());
        };

        // The path is a parameter-name prefix, so it carries the name rules: trimmed,
        // no spaces between characters, and a leading forward slash.
        let path =
            parameter_path(&input.path).map_err(crate::scaffolded::Scaffolded::validation)?;

        let filters = input.parameter_filters.as_deref().unwrap_or_default();
        for filter in filters {
            if !is_parameters_by_path_filter_key(&filter.key) {
                // The modelled shape declares no members, yet the service words this
                // refusal: it names the rejected key and the keys a hierarchy read
                // accepts, which are fewer than a describe's.
                return Err(crate::scaffolded::Scaffolded::invalid_filter_key(
                    invalid_filter_key_message(&filter.key, PARAMETERS_BY_PATH_FILTER_KEYS),
                ));
            }
            // "Valid options include Equals and BeginsWith" — and a filter naming none
            // compares whole values.
            let option = filter.option.as_deref().unwrap_or(FILTER_OPTION_EQUALS);
            let allowed = parameters_by_path_filter_options(&filter.key);
            if !allowed.contains(&option) {
                return Err(GetParametersByPathErrorModeled::InvalidFilterOption(
                    InvalidFilterOption {
                        message: Some(invalid_filter_option_message(option, allowed)),
                    },
                )
                .into());
            }
            let values = filter.values.as_deref().unwrap_or_default();
            if values.is_empty() {
                return Err(invalid_filter_value(format!(
                    "Filter key '{}' carries no value to search for",
                    filter.key
                )));
            }
            // A parameter's type is one of the modelled `ParameterType` values, so an
            // equality filter on a value outside that enum asks for a type no parameter
            // can have. A `BeginsWith` value is a prefix of a type, not a type, so it is
            // not checked against the enum.
            if filter.key == FILTER_KEY_TYPE && option == FILTER_OPTION_EQUALS {
                if let Some(unknown) = values
                    .iter()
                    .find(|value| !PARAMETER_TYPE_VALUES.contains(&value.as_str()))
                {
                    return Err(invalid_filter_value(format!(
                        "Value '{unknown}' at filter key '{}' isn't a parameter type: valid \
                         values are {}",
                        filter.key,
                        PARAMETER_TYPE_VALUES.join(", ")
                    )));
                }
            }
        }

        let page_size = parameters_by_path_page_size(input.max_results)
            .map_err(crate::scaffolded::Scaffolded::validation)?;

        // The token is this read's own continuation token, which carries the name the next
        // page starts at behind an authenticator only this store can produce
        // ([`crate::state::parameter_pagination`]) — so a value this read did not issue
        // names no position at all, however exactly it is shaped like one, and is refused
        // rather than silently restarting the scan.
        let page_key = state.lock().page_token_key();
        let resume_from = match input.next_token.as_deref() {
            None | Some("") => None,
            Some(token) => match page_cursor(&page_key, PagedRead::Hierarchy, token) {
                Some(cursor) => Some(cursor),
                None => {
                    return Err(GetParametersByPathErrorModeled::InvalidNextToken(
                        InvalidNextToken {
                            message: Some(invalid_next_token_message(token)),
                        },
                    )
                    .into())
                }
            },
        };

        // Parameter Store is regional: a parameter is reached through the endpoint it
        // was created against, which is the Region this request was signed for. The
        // store yields that Region's parameters in ascending canonical-name order, which
        // is what makes the resume cursor total.
        let recursive = input.recursive.unwrap_or(false);
        let region = request_region(ctx.parts());
        // AWS's own published parameters are read by naming the hierarchy AWS owns: no
        // parameter of this account can live in it (`PutParameter` refuses such a name)
        // and AWS publishes nothing in the account's own hierarchy, so a read of AWS's
        // hierarchy reports the published catalog and every other path reports what this
        // account wrote. Projected BEFORE the store guard is taken, so no state of this
        // service is held while the catalog's own store is read.
        let published = if crate::state::public_parameters::is_published_hierarchy(path) {
            let published =
                crate::state::public_parameters::published_records(&*self.engine, &region);
            // Inside AWS's hierarchy a read reaches the NAMESPACES AWS publishes in, and
            // nothing else: no parameter of this account can live there, so a path that is
            // no namespace of it names nothing any caller could reach — which is a refusal
            // and not an empty page.
            if !crate::state::public_parameters::is_reserved_namespace(path, &published) {
                return Err(GetParametersByPathErrorModeled::AccessDeniedException(
                    AccessDeniedException {
                        message: crate::state::public_parameters::invalid_namespace_message(
                            path,
                        ),
                    },
                )
                .into());
            }
            published
        } else {
            Vec::new()
        };
        // The store guard is released before the keys are asked, so no state of this
        // service is held while KMS's decision is made.
        let (mut parameters, next_token, to_open) = {
            let store = state.lock();
            let mut parameters = Vec::new();
            let mut to_open = Vec::new();
            let mut next_token = None;
            // One ascending order over both sources, so a page boundary and the name it
            // resumes at mean the same thing whichever source the parameter came from.
            let mut candidates: Vec<(&str, &crate::state::ParameterRecord)> = store
                .parameters_of_region(&region)
                .chain(
                    published
                        .iter()
                        .map(|record| (canonical_name(&record.name), record)),
                )
                .collect();
            candidates.sort_by_key(|(name, _)| *name);
            for (name, record) in candidates {
                if resume_from.as_deref().is_some_and(|from| name < from) {
                    continue;
                }
                if !is_under_path(name, path, recursive) {
                    continue;
                }
                let Some(version) = select_version(record, filters) else {
                    continue;
                };
                if !filters.iter().all(|filter| version_matches(version, filter)) {
                    continue;
                }
                if parameters.len() == page_size {
                    // The page is full and a parameter is still waiting: hand the caller
                    // a token naming the parameter it resumes at, so the next request
                    // continues where this stopped.
                    next_token = Some(page_token(
                        &page_key,
                        PagedRead::Hierarchy,
                        canonical_name(&record.name),
                    ));
                    break;
                }
                // The request named no `name:selector`, so the reported parameter carries
                // none: a selector is "either the version number or the label used to
                // retrieve the parameter value" (`Parameter$Selector`).
                parameters.push(version.to_parameter(record, None));
                to_open.extend(
                    crate::state::secure_string::value_to_open(
                        input.with_decryption,
                        record,
                        version,
                    )
                    .map(|sealed| (parameters.len() - 1, sealed)),
                );
            }
            (parameters, next_token, to_open)
        };

        // A `WithDecryption` read has each reported `SecureString` opened under that
        // parameter's own KMS key, so every one of those keys decides the read before any
        // value is reported; a read that asks for none reports the stored ciphertexts.
        for (at, plaintext) in
            crate::state::secure_string::open(&*self.engine, ctx, &region, &to_open).map_err(
                |refusal| match refusal {
                    crate::state::secure_string::OpenRefusal::Denied(message) => {
                        GetParametersByPathErrorModeled::AccessDeniedException(
                            AccessDeniedException { message },
                        )
                    }
                    crate::state::secure_string::OpenRefusal::Key(message) => {
                        GetParametersByPathErrorModeled::InvalidKeyId(InvalidKeyId {
                            message: Some(message),
                        })
                    }
                },
            )?
        {
            if let Some(reported) = parameters.get_mut(at) {
                reported.value = Some(plaintext);
            }
        }

        Ok(GetParametersByPathOutput {
            parameters: Some(parameters),
            next_token,
        })
    }
}

/// The version of `record` a hierarchy read reports, or `None` when the read's filters
/// name a label no version of this parameter carries.
///
/// A label points at ONE version of a parameter ("A label can be assigned to only one
/// version of a parameter at a time", `LabelParameterVersion`), so a read filtered on a
/// label reports the labelled version rather than the newest. With no label filter the
/// newest version is what the read reports, as a read with no selector does.
fn select_version<'a>(
    record: &'a crate::state::ParameterRecord,
    filters: &[ParameterStringFilter],
) -> Option<&'a crate::state::ParameterVersionRecord> {
    let labels: Vec<&str> = filters
        .iter()
        .filter(|filter| filter.key == crate::state::FILTER_KEY_LABEL)
        .flat_map(|filter| filter.values.as_deref().unwrap_or_default())
        .map(String::as_str)
        .collect();
    if labels.is_empty() {
        return record.latest();
    }
    record
        .versions
        .iter()
        .rev()
        .find(|version| labels.iter().any(|label| version.labels.contains(*label)))
}

/// Whether `version` carries a value one filter asks for. A key that names no attribute
/// of a parameter version matches nothing — such a key is already refused above, so that
/// arm is a backstop.
fn version_matches(
    version: &crate::state::ParameterVersionRecord,
    filter: &ParameterStringFilter,
) -> bool {
    let Some(held) = version.filter_values(&filter.key) else {
        return false;
    };
    crate::state::string_filter_matches(
        &held,
        filter
            .option
            .as_deref()
            .unwrap_or(crate::state::FILTER_OPTION_EQUALS),
        filter.values.as_deref().unwrap_or_default(),
    )
}

/// The modelled refusal for a filter value this operation cannot search on: "The filter
/// value isn't valid. Verify the value and try again".
fn invalid_filter_value(message: String) -> GetParametersByPathError {
    GetParametersByPathErrorModeled::InvalidFilterValue(InvalidFilterValue {
        message: Some(message),
    })
    .into()
}
