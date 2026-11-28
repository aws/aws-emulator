// SPDX-License-Identifier: CC0-1.0

//! The refusals the LEGACY Greengrass surface answers with, and the error channel that
//! carries them.
//!
//! Four of this service's operations are bound to routes the 2017-06-07 Greengrass model
//! binds too — `GET`/`PUT`/`DELETE /greengrass/servicerole` and
//! `GET`/`PUT /greengrass/things/{thingName}/connectivityInfo`
//! (`smithy_query(query="digest")` on both models; `service_common::greengrass` names this
//! service as the one claimant). AWS co-hosts the two API versions on one endpoint and
//! answers those routes in the ELDER envelope: a client error carries its sentence in a
//! `Message` body member, not in the lowercase `message` the 2020-11-30 shapes declare, it
//! names NO error shape at all (so the code every SDK reports for it is the HTTP status),
//! and a legacy fact that is not established — the account's service role, a core device's
//! connectivity information — is reported as a 404 rather than as an empty success. None of
//! that is expressible as a variant of an operation's `#[operation_error]` enum: the body
//! member's wire name belongs to the shape, no shape of this model carries a 404 at all,
//! and every modelled failure names its shape.
//!
//! So the two refusal classes live here, once, rather than in each operation's file: what
//! a refusal looks like on the wire is a property of the SERVICE, and two operations that
//! reach the same decision must not word it two ways. An operation carries them by
//! declaring its error channel [`LegacyRefusable`], which widens that operation's own
//! modelled channel without changing how any modelled error is rendered.
//!
//! The sibling class [`super::TaggingNotFound`] is the OTHER recorded 404 of this service
//! and keeps its own lowercase `message`: it answers the 2020-11-30 tagging route, whose
//! recorded envelope is this model's own. The two differ because the recorded responses
//! differ, not because two authors chose differently.

use ::aws_smithy_runtime_api::http::Response as WireResponse;
use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};
use ::service_common::error_type_locator::{without_error_shape_in_body, ERROR_TYPE_HEADER};
use ::smithy_server_runtime::OperationError;

/// The status a client-input refusal of the legacy surface carries: the `httpError` of the
/// client-error shape the 2017-06-07 model declares for both mutating operations of that
/// surface (`smithy_query(query="operation",operation="AssociateServiceRoleToAccount")` on
/// `com-amazonaws-greengrass-greengrass` lists `BadRequestException`, `httpError` 400, whose
/// one documented member is `Message`).
pub const LEGACY_BAD_REQUEST_STATUS: u16 = 400;

/// The status a "that fact is not established" report of the legacy surface carries.
///
/// RECORDED behaviour, not modelled: neither Greengrass model declares a 404 shape for the
/// account service role or for a core device's connectivity information, yet the endpoint
/// answers both at that status, with a sentence saying what is missing.
pub const LEGACY_NOT_FOUND_STATUS: u16 = 404;

/// What a client reads as the error CODE of a legacy refusal: its HTTP status, as text.
///
/// RECORDED behaviour: an answer of this surface names no error shape — the body carries the
/// one `Message` member and no shape discriminator, and no shape header travels with it — so
/// every SDK falls back to the status string for the code a client branches on.
/// [`without_error_shape`] is what holds that true of the RENDERED answer; these two are the
/// code that answer then surfaces as, and are what the error channel reports as its shape
/// name, so a refusal is classified under what its client sees rather than under a shape no
/// response names.
pub const LEGACY_BAD_REQUEST_CODE: &str = "400";

/// The code a not-found report of this surface surfaces as, for the same reason.
pub const LEGACY_NOT_FOUND_CODE: &str = "404";

/// What `AssociateServiceRoleToAccount` answers a `RoleArn` that is no IAM role ARN.
///
/// Fixed text: the refusal is about the FORM of the value, so it quotes nothing back.
pub const INVALID_ROLE_ARN_MESSAGE: &str = "That role ARN is invalid.";

/// What `GetServiceRoleForAccount` answers while the account has no service role.
///
/// Fixed text: it states the action the caller has to take, and names no value of the
/// request (there is none to name — the operation takes no input).
pub const NO_SERVICE_ROLE_MESSAGE: &str = "You need to attach an IAM role to this AWS account.";

/// What `UpdateConnectivityInfo` answers a request whose endpoint list it READ and could
/// not take as connectivity information — an entry of it names no endpoint.
///
/// Fixed text: the refusal is about the request BODY, so it names no member, value or limit
/// of the request.
pub const UNREADABLE_REQUEST_BODY_MESSAGE: &str = "Invalid json format found in the request body.";

/// The name the service publishes `UpdateConnectivityInfo`'s endpoint list under, which is
/// what [`unparsable_parameter_message`] names when nothing readable as that list arrived:
/// for a list-valued member it is the name of the LIST SHAPE the member targets
/// (`smithy_query(query="operation",operation="UpdateConnectivityInfo")`: `connectivityInfo`
/// targets `connectivityInfoList`), not the member name and not a value of the request.
pub const CONNECTIVITY_INFO_PARAMETER: &str = "connectivityInfoList";

/// What the legacy surface answers a request under whose parameters nothing it could read as
/// those parameters' values arrived: the fixed sentence, naming every one of them.
///
/// The sentence's fixed text is everything but the names, and its variable part is the list
/// of parameters the service could not parse, taken from the request — so a caller is told
/// exactly which parameters of its own request it has to correct. Several are stated in one
/// sentence, which is what the `(s)` of the fixed text is for, separated as a list of names
/// is read.
///
/// The two refusals are distinct classes, worded as AWS words each: this one names the
/// parameters nothing readable arrived under, and [`UNREADABLE_REQUEST_BODY_MESSAGE`] answers
/// a parameter that WAS read and carries something the operation does not take.
///
/// # Panics
///
/// In a debug build, when `parameters` is empty: the sentence exists to NAME what a caller
/// has to correct, so a refusal that names nothing is a caller being told nothing, and the
/// decision to refuse at all is the decision that there is a name to state.
#[must_use]
pub fn unparsable_parameter_message(parameters: &[&str]) -> String {
    debug_assert!(
        !parameters.is_empty(),
        "this refusal names the parameters it could not parse, so there is at least one"
    );
    format!(
        "The following required parameter(s) are unparsable: {}.",
        parameters.join(PARAMETER_SEPARATOR)
    )
}

/// What separates two parameter names in [`unparsable_parameter_message`].
const PARAMETER_SEPARATOR: &str = ", ";

/// What `GetConnectivityInfo` answers for a core device that has published no connectivity
/// information.
///
/// The thing name the request carried is the one variable part of the sentence; the rest is
/// fixed text, and nothing of the stored state appears in it — there is none to report.
#[must_use]
pub fn no_connectivity_info_message(thing_name: &str) -> String {
    format!("No connectivity info found for thing {thing_name}.")
}

/// The `Message` member of the legacy client-error shape.
///
/// Spelled `Message` on the wire (`with_json_name`): the recorded legacy envelope carries
/// its sentence under that capitalized key, and a client reading it finds nothing under
/// any other name. `with_xml_name` matches, so the shape renders the same on a protocol
/// this service does not speak.
static LEGACY_BAD_REQUEST_MESSAGE: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#GreengrassLegacyBadRequest$Message",
        "smithy.framework",
        "Message",
    ),
    ShapeType::String,
    "Message",
    0,
)
.with_xml_name("Message")
.with_json_name("Message");

/// Schema for the legacy client-error shape.
///
/// In the `smithy.framework` namespace for the same reason as the scaffold's synthetic
/// shapes (`crate::scaffolded`): the shape is in no model, so it must not collide with a
/// modelled one, and its own id never reaches the wire. It is what [`without_error_shape`]
/// recognises a rendered answer of this surface by, so no shape — neither this one nor the
/// code [`LEGACY_BAD_REQUEST_CODE`] stands in for — is named in the response.
static LEGACY_BAD_REQUEST_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#GreengrassLegacyBadRequest",
        "smithy.framework",
        "GreengrassLegacyBadRequest",
    ),
    ShapeType::Structure,
    &[&LEGACY_BAD_REQUEST_MESSAGE],
);

/// The `Message` member of the legacy not-found shape. The same wire name as
/// [`LEGACY_BAD_REQUEST_MESSAGE`], on its own shape so the two refusals stay distinct
/// shapes with distinct statuses.
static LEGACY_NOT_FOUND_MESSAGE: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#GreengrassLegacyNotFound$Message",
        "smithy.framework",
        "Message",
    ),
    ShapeType::String,
    "Message",
    0,
)
.with_xml_name("Message")
.with_json_name("Message");

/// Schema for the legacy not-found shape. Synthetic for the same reason as
/// [`LEGACY_BAD_REQUEST_SCHEMA`].
static LEGACY_NOT_FOUND_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#GreengrassLegacyNotFound",
        "smithy.framework",
        "GreengrassLegacyNotFound",
    ),
    ShapeType::Structure,
    &[&LEGACY_NOT_FOUND_MESSAGE],
);

/// Whether a failure rendered against `error_schema` is one of this surface's own refusals.
///
/// The two synthetic shapes above are what mark it: a MODELLED failure of these operations
/// renders against its model's own shape and keeps naming it.
#[must_use]
fn is_legacy_refusal(error_schema: &Schema) -> bool {
    [&LEGACY_BAD_REQUEST_SCHEMA, &LEGACY_NOT_FOUND_SCHEMA]
        .into_iter()
        .any(|shape| shape.shape_id() == error_schema.shape_id())
}

/// `response`, rendered for a failure against `error_schema`, as the LEGACY surface answers
/// it: a refusal of either class above carries its one `Message` and names no error shape —
/// neither the body's discriminator nor the header a REST client resolves a shape from — so
/// the code every SDK reports for it is the HTTP status it carries.
///
/// A failure of any other class is returned untouched, status, headers and body alike: a
/// modelled error of these same operations is named by its modelled shape exactly as its
/// siblings' are.
#[must_use]
pub fn without_error_shape(error_schema: &Schema, response: WireResponse) -> WireResponse {
    if !is_legacy_refusal(error_schema) {
        return response;
    }
    let mut response = without_error_shape_in_body(response);
    response.headers_mut().remove(ERROR_TYPE_HEADER);
    response
}

/// The body of a legacy client-error refusal: the one sentence it states.
#[derive(Debug, Clone)]
pub struct LegacyBadRequest {
    pub message: String,
}

impl SerializableStruct for LegacyBadRequest {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&LEGACY_BAD_REQUEST_MESSAGE, self.message.as_str())?;
        Ok(())
    }
}

/// The body of a legacy not-found report: the one sentence it states.
#[derive(Debug, Clone)]
pub struct LegacyNotFound {
    pub message: String,
}

impl SerializableStruct for LegacyNotFound {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&LEGACY_NOT_FOUND_MESSAGE, self.message.as_str())?;
        Ok(())
    }
}

/// A refusal of the legacy surface, carrying the ONE mapping from the refusal class to the
/// wire code, the HTTP status and the body it answers with.
#[derive(Debug)]
pub enum LegacyRefusal {
    /// A request this surface cannot read: the body, or a value it carries, is not of the
    /// form the operation takes.
    BadRequest(LegacyBadRequest),
    /// The fact the request asked about is not established — for the account, or for the
    /// core device the request named.
    NotFound(LegacyNotFound),
}

impl LegacyRefusal {
    /// A client-input refusal carrying `message`.
    #[must_use]
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(LegacyBadRequest {
            message: message.into(),
        })
    }

    /// A not-found report carrying `message`.
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(LegacyNotFound {
            message: message.into(),
        })
    }

    /// The body this refusal serializes.
    fn as_serializable(&self) -> &dyn SerializableStruct {
        match self {
            Self::BadRequest(body) => body,
            Self::NotFound(body) => body,
        }
    }

    /// The schema the answer is written against.
    fn variant_schema(&self) -> &'static Schema {
        match self {
            Self::BadRequest(_) => &LEGACY_BAD_REQUEST_SCHEMA,
            Self::NotFound(_) => &LEGACY_NOT_FOUND_SCHEMA,
        }
    }

    /// The code this refusal SURFACES as, which the framework reads from an operation's
    /// `shape_name`. The rendered answer names no shape ([`without_error_shape`]), so this
    /// is the status the client falls back to rather than a shape name.
    fn error_code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => LEGACY_BAD_REQUEST_CODE,
            Self::NotFound(_) => LEGACY_NOT_FOUND_CODE,
        }
    }

    /// The HTTP status this refusal answers with.
    fn http_status(&self) -> u16 {
        match self {
            Self::BadRequest(_) => LEGACY_BAD_REQUEST_STATUS,
            Self::NotFound(_) => LEGACY_NOT_FOUND_STATUS,
        }
    }

    /// The sentence the body states, for the [`std::fmt::Display`] rendering below.
    fn message(&self) -> &str {
        match self {
            Self::BadRequest(body) => &body.message,
            Self::NotFound(body) => &body.message,
        }
    }
}

impl ::std::fmt::Display for LegacyRefusal {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(f, "{}: {}", self.error_code(), self.message())
    }
}

impl ::std::error::Error for LegacyRefusal {}

/// One legacy operation's error channel, widened by the refusals above.
///
/// `Channel` is the operation's own channel — its modelled errors, the scaffold's 501 and
/// the framework's synthetic `ValidationException` — rendered exactly as it is rendered
/// without this wrapper. `Refused` is a refusal this surface words itself. An operation
/// declares `type Error = LegacyRefusable<{Op}ErrorModeled>` and keeps writing
/// `Err(SomeModeledError.into())`.
#[derive(Debug)]
pub enum LegacyRefusable<E> {
    /// The operation's own error channel.
    Channel(crate::scaffolded::Scaffolded<E>),
    /// A refusal the legacy surface words itself.
    Refused(LegacyRefusal),
}

impl<E> From<crate::scaffolded::Scaffolded<E>> for LegacyRefusable<E> {
    fn from(channel: crate::scaffolded::Scaffolded<E>) -> Self {
        Self::Channel(channel)
    }
}

impl<E> From<LegacyRefusal> for LegacyRefusable<E> {
    fn from(refusal: LegacyRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl<E: ::std::fmt::Display> ::std::fmt::Display for LegacyRefusable<E> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self {
            Self::Channel(error) => write!(f, "{error}"),
            Self::Refused(refusal) => write!(f, "{refusal}"),
        }
    }
}

/// `source()` forwards to the wrapped failure rather than taking the default `None`.
impl<E: ::std::error::Error + 'static> ::std::error::Error for LegacyRefusable<E> {
    fn source(&self) -> Option<&(dyn ::std::error::Error + 'static)> {
        match self {
            Self::Channel(error) => error.source(),
            Self::Refused(refusal) => refusal.source(),
        }
    }
}

/// Every method delegates: the operation's own channel renders a modelled error exactly as
/// it would without this wrapper, and a refusal renders as [`LegacyRefusal`] states it — so
/// an operation that carries this channel answers either condition exactly as its siblings
/// do.
impl<E: OperationError> OperationError for LegacyRefusable<E> {
    fn as_serializable(&self) -> &dyn SerializableStruct {
        match self {
            Self::Channel(error) => error.as_serializable(),
            Self::Refused(refusal) => refusal.as_serializable(),
        }
    }

    fn variant_schema(&self) -> &'static Schema {
        match self {
            Self::Channel(error) => error.variant_schema(),
            Self::Refused(refusal) => refusal.variant_schema(),
        }
    }

    fn shape_name(&self) -> &'static str {
        match self {
            Self::Channel(error) => error.shape_name(),
            Self::Refused(refusal) => refusal.error_code(),
        }
    }

    fn http_status(&self) -> u16 {
        match self {
            Self::Channel(error) => error.http_status(),
            Self::Refused(refusal) => refusal.http_status(),
        }
    }

    fn query_error_code(&self) -> Option<&'static str> {
        match self {
            Self::Channel(error) => error.query_error_code(),
            // The refusal's shape is in no model, so it carries no `awsQueryError` trait
            // to honour — and this service speaks only `restJson1`.
            Self::Refused(_) => None,
        }
    }
}
