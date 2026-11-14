// SPDX-License-Identifier: CC0-1.0

//! The Amazon SimpleDB 2009-04-15 AWS Query API, served on this endpoint beside the v2
//! export operations.
//!
//! # Why this surface lives here
//!
//! The `sdb` endpoint publishes TWO APIs. The v2 export operations are modelled
//! (`smithy_query(query="digest")` reports `restJson1` and three operations bound to
//! `/v2/*`), and the Amazon SimpleDB 2009-04-15 API is not modelled in this repo at all:
//! it is an AWS Query API served on `"/"` as a form-encoded request carrying
//! `Action=<action>&Version=2009-04-15` plus that action's parameters. Both are addressed
//! with the same `aws.api#service` endpoint prefix and the same `aws.auth#sigv4` signing
//! name (`sdb`), so one service claims both and the request's own shape decides which API
//! it is: a request on `"/"` naming an `Action` is this one.
//!
//! Nothing in the generated routing table can name such a request — the table is keyed by
//! the modelled `method + URI`, and `POST /` matches none of the three — so it is routed
//! through [`smithy_server_runtime::Service::override_operation`], the one seam that sees
//! the request BODY before the model's routing key is resolved. That seam names it an
//! operation, which is what makes the router verify the caller's signature before
//! anything is served: this is an AWS API, reached with AWS credentials, and it is
//! authenticated exactly as the modelled operations are.
//!
//! # What is served
//!
//! The domain surface and the attribute surface, each from the ONE store the export
//! operations read ([`crate::state::simpledb_domains`]), so a domain `CreateDomain`
//! makes is a domain `StartDomainExport` resolves and the items `PutAttributes` writes
//! are the items an export of that domain counts:
//!
//! * `CreateDomain` — creates the domain, and is idempotent on a name the account
//!   already holds;
//! * `DeleteDomain` — removes the domain and its items, and is a no-op on a name the
//!   account does not hold;
//! * `ListDomains` — pages through the names the account holds;
//! * `DomainMetadata` — reports one domain's counts and sizes, computed from its items;
//! * `PutAttributes` — writes one item's attribute name-value pairs, replacing or adding
//!   per pair, under the request's `Expected` conditions;
//! * `GetAttributes` — reports one item's pairs, optionally filtered to named
//!   attributes;
//! * `DeleteAttributes` — deletes pairs, whole attributes, or the whole item, under the
//!   same `Expected` conditions.
//!
//! [`UNSERVED_ACTIONS`] are the remaining actions of the same API. Each is answered
//! `501` rather than with a success or with `InvalidAction`: the action does exist on
//! this endpoint, so naming it invalid would be a false statement about the service,
//! while a `200` would report an interaction nothing performed. Each is its own gap, to
//! be implemented against its own recorded behaviour — `Select` needs the API's query
//! language, and the two `Batch*` actions need the multi-item envelope.

use std::sync::Arc;
use std::time::SystemTime;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

use envelope::QueryApiError;
use parameters::QueryParameters;

/// The domain actions: `CreateDomain`, `DeleteDomain`, `ListDomains`, `DomainMetadata`.
pub mod domains;

/// The documents this API answers in, and the refusals it states.
pub mod envelope;

/// The item actions: `PutAttributes`, `GetAttributes`, `DeleteAttributes`.
pub mod items;

/// The parameters of one request, read off the form it arrived in.
pub mod parameters;

/// The API version this surface serves, as its requests name it.
pub const API_VERSION: &str = "2009-04-15";

/// The parameter naming the action a request calls.
const ACTION_PARAMETER: &str = "Action";

/// The parameter naming the API version a request is for.
const VERSION_PARAMETER: &str = "Version";

/// The parameter naming the domain an action is about.
pub const DOMAIN_NAME_PARAMETER: &str = "DomainName";

/// The parameter naming the item an action is about.
pub const ITEM_NAME_PARAMETER: &str = "ItemName";

/// The actions of this API this emulator does not serve yet. See the module docs for why
/// each is a `501` rather than a success or an `InvalidAction`.
pub const UNSERVED_ACTIONS: &[&str] = &["BatchDeleteAttributes", "BatchPutAttributes", "Select"];

/// The auth schemes a request to this surface is verified under.
///
/// SigV4 is the only scheme this service's model declares, and this surface is reached
/// with the same AWS credentials the modelled operations are. It is NOT a modelled
/// operation, so there is no modelled operation for the router to read schemes off:
/// without this the router would find no scheme for such a request and could not verify
/// the credential it carries.
const QUERY_API_AUTH_SCHEMES: &[&str] = &[::emulator_server::aws_credentials::SIGV4_SCHEME];

/// Whether `req` addresses this API: a form-encoded request naming an `Action`, either on
/// the endpoint's root path or naming this API's own `Version`.
///
/// Two signals, because this endpoint carries two APIs and a client of either may address
/// it in more than one way. The ROOT PATH is where the AWS Query API lives, and the
/// modelled operations are bound under `/v2/`, so an `Action` on `"/"` is this API's.
/// `Version=2009-04-15` is the API's own identifier, which its requests carry whatever
/// path they reach the endpoint on, so a request stating it is this API's too. A modelled
/// request can match neither: its body is a JSON document, which carries no form field at
/// all.
pub fn addresses_query_api(req: &RtRequest) -> bool {
    let parameters = QueryParameters::of(req);
    let names_action = parameters
        .value(ACTION_PARAMETER)
        .is_some_and(|action| !action.trim().is_empty());
    if !names_action {
        return false;
    }
    let path = ::service_common::uri_path(req.uri());
    path.is_empty()
        || path == "/"
        || parameters.value(VERSION_PARAMETER) == Some(API_VERSION)
}

/// The Amazon SimpleDB 2009-04-15 API, as the one [`ErasedOperation`] the service holds
/// for it.
///
/// Owns the shared engine exactly as a generated operation does, so every answer is
/// computed from the SAME per-service store the v2 export handlers read and write.
pub struct SimpleDbQueryApi<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> SimpleDbQueryApi<Eng> {
    /// Build this surface from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for SimpleDbQueryApi<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { self.serve(req, ctx) })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(QUERY_API_AUTH_SCHEMES)
    }
}

impl<Eng: Send + Sync + 'static> SimpleDbQueryApi<Eng> {
    /// Serve one request to this API: the `Action` field names the action, and that
    /// action's own handler answers it from this service's store.
    ///
    /// ONE instant answers the whole request, and ONE request id names it in both the
    /// document and the response header.
    pub fn serve(&self, req: &RtRequest, ctx: &OperationContext<'_>) -> RtResponse {
        let request_id = ::service_common::query_error_envelope::new_request_id();
        let parameters = QueryParameters::of(req);
        let now = SystemTime::now();
        let action = parameters.value(ACTION_PARAMETER).unwrap_or_default().trim();
        let served = self.answer(action, &parameters, now, &request_id);
        match served {
            Ok(response) => {
                audit(ctx, action, "served");
                response
            }
            Err(refusal) => {
                audit(ctx, action, refusal.code);
                envelope::error_response(&refusal, &request_id)
            }
        }
    }

    /// The answer for `action`, or the refusal that stands in its place.
    ///
    /// The version is checked first and for every action: a request naming a version no
    /// Amazon SimpleDB API declares is for an API this endpoint does not serve, whatever
    /// action it goes on to name. A request naming NO version has stated nothing to
    /// disagree with and is served as this API's own version, which is the only one here.
    fn answer(
        &self,
        action: &str,
        parameters: &QueryParameters,
        now: SystemTime,
        request_id: &str,
    ) -> Result<RtResponse, QueryApiError> {
        if action.is_empty() {
            return Err(envelope::missing_action());
        }
        if let Some(version) = parameters.value(VERSION_PARAMETER) {
            if version != API_VERSION {
                return Err(envelope::no_such_version(version));
            }
        }
        match action {
            "CreateDomain" => self.create_domain(parameters, request_id),
            "DeleteDomain" => self.delete_domain(parameters, request_id),
            "ListDomains" => self.list_domains(parameters, request_id),
            "DomainMetadata" => self.domain_metadata(parameters, request_id),
            "PutAttributes" => self.put_attributes(parameters, now, request_id),
            "GetAttributes" => self.get_attributes(parameters, request_id),
            "DeleteAttributes" => self.delete_attributes(parameters, now, request_id),
            unserved if UNSERVED_ACTIONS.contains(&unserved) => {
                Err(envelope::not_implemented(unserved))
            }
            unknown => Err(envelope::invalid_action(unknown)),
        }
    }

    /// This service's store, or the refusal to send when the engine this surface was
    /// built over does not hold one.
    ///
    /// An engine without this service's store is "nobody wired that engine up", not a
    /// request failure: it is the server's own, so it is stated as one rather than as a
    /// fabricated success.
    fn store(
        &self,
    ) -> Result<
        ::engine::ServiceState<crate::state::ComAmazonawsSimpledbv2Simpledbv2State>,
        QueryApiError,
    > {
        crate::state::service_state(&*self.engine).ok_or_else(envelope::internal_error)
    }
}

/// The `DomainName` the request names, refusing a parameter that is absent or that this
/// API does not admit — the one reader every action that takes a domain goes through, so
/// no two of them judge a domain name differently.
pub fn required_domain_name(parameters: &QueryParameters) -> Result<&str, QueryApiError> {
    let value = parameters
        .value(DOMAIN_NAME_PARAMETER)
        .ok_or_else(|| envelope::missing_parameter(DOMAIN_NAME_PARAMETER))?;
    match domain_name_refusal(value) {
        Some(refusal) => Err(refusal),
        None => Ok(value),
    }
}

/// Why this API does not admit `value` as a domain name, or `None` when it does.
///
/// Amazon SimpleDB documents the rule: a domain name is 3 to 255 characters long and
/// carries only `a-z`, `A-Z`, `0-9`, `_`, `-` and `.`. The whole rule is stated in the
/// refusal, because what the caller has to fix is the rule rather than the one way this
/// value missed it.
fn domain_name_refusal(value: &str) -> Option<QueryApiError> {
    let length = value.chars().count();
    let within_bounds = (crate::state::simpledb_domains::MIN_DOMAIN_NAME_LEN
        ..=crate::state::simpledb_domains::MAX_DOMAIN_NAME_LEN)
        .contains(&length);
    let admitted = value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'));
    (!within_bounds || !admitted).then(|| {
        envelope::invalid_parameter_value(
            DOMAIN_NAME_PARAMETER,
            value,
            &format!(
                "{DOMAIN_NAME_PARAMETER} must be between {min} and {max} characters long and \
                 may contain only a-z, A-Z, 0-9, '_', '-' and '.'.",
                min = crate::state::simpledb_domains::MIN_DOMAIN_NAME_LEN,
                max = crate::state::simpledb_domains::MAX_DOMAIN_NAME_LEN,
            ),
        )
    })
}

/// The `ItemName` the request names, refusing a parameter that is absent or longer than
/// this API admits.
pub fn required_item_name(parameters: &QueryParameters) -> Result<&str, QueryApiError> {
    let value = parameters
        .value(ITEM_NAME_PARAMETER)
        .ok_or_else(|| envelope::missing_parameter(ITEM_NAME_PARAMETER))?;
    within_text_bound(ITEM_NAME_PARAMETER, value)?;
    Ok(value)
}

/// Refuse `value` for `parameter` when it is longer than the bytes this API admits for an
/// item name, an attribute name or an attribute value.
///
/// Amazon SimpleDB documents 1024 bytes for each of the three, and states the refusal
/// against the maximum the value exceeded.
pub fn within_text_bound(parameter: &str, value: &str) -> Result<(), QueryApiError> {
    if value.len() > crate::state::simpledb_domains::MAX_TEXT_BYTES {
        return Err(envelope::invalid_parameter_value(
            parameter,
            value,
            &format!(
                "Value exceeds maximum length of {}.",
                crate::state::simpledb_domains::MAX_TEXT_BYTES
            ),
        ));
    }
    Ok(())
}

/// How a refusal the STORE raised is worded on this wire.
///
/// The store states what it refused structurally ([`crate::state::simpledb_domains::WriteRefusal`]);
/// the sentence is this module's, so one refusal cannot be worded two ways by the two
/// actions that can raise it.
pub fn write_refusal(refusal: crate::state::simpledb_domains::WriteRefusal) -> QueryApiError {
    use crate::state::simpledb_domains::WriteRefusal;
    match refusal {
        WriteRefusal::NoSuchDomain => envelope::no_such_domain(),
        WriteRefusal::ItemAttributesExceeded => envelope::number_item_attributes_exceeded(),
        WriteRefusal::DomainBytesExceeded => envelope::number_domain_bytes_exceeded(),
    }
}

/// Audit record for one call on this surface: which action was asked for, and how it
/// ended.
///
/// Emitted at INFO so it survives the emulator's default log filtering. Only the action
/// — one of this crate's own literals — the outcome and the caller's access-key id go in:
/// no domain name, no item name and no attribute text, so nothing a caller sent can
/// forge a line of this record.
fn audit(ctx: &OperationContext<'_>, action: &str, outcome: &str) {
    let action = if action.is_empty() { "<none>" } else { action };
    let action = match SERVED_ACTIONS
        .iter()
        .chain(UNSERVED_ACTIONS)
        .find(|known| **known == action)
    {
        Some(known) => *known,
        None => "<unknown>",
    };
    ::tracing::info!(
        audit = true,
        operation = "SimpleDB2009-04-15",
        action,
        outcome,
        caller_akid = %::engine::extract_akid(&ctx.parts().headers),
        "SimpleDB {action} {outcome}"
    );
}

/// The actions this surface serves, in the spelling its own handlers are named for.
///
/// It is the vocabulary [`audit`] reports an action by, so a caller-supplied `Action` is
/// never written to a log line verbatim.
pub const SERVED_ACTIONS: &[&str] = &[
    "CreateDomain",
    "DeleteAttributes",
    "DeleteDomain",
    "DomainMetadata",
    "GetAttributes",
    "ListDomains",
    "PutAttributes",
];
