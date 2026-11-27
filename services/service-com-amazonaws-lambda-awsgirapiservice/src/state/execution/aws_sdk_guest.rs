// SPDX-License-Identifier: CC0-1.0

//! The AWS SDK a Python handler imports: the `boto3` and `botocore` modules this runtime
//! supplies, in Python, over the one host function that makes a call
//! ([`super::aws_api`]).
//!
//! AWS Lambda's managed `python3.x` runtimes ship the AWS SDK for Python on the function's
//! import path, so `import boto3` at module scope succeeds and the handler can call another
//! service without shipping a dependency of its own. These modules are that import path's
//! contents here. They are the SDK's CLIENT interface — `boto3.client(...)`, an operation
//! per method, `botocore.exceptions.ClientError` for a refusal — because that is the
//! interface a handler calls a service through.
//!
//! What they do and do not do:
//!
//! * A method name is turned into the operation it names (`send_message` ->
//!   `SendMessage`) and its keyword arguments ARE the operation's input document, which is
//!   what an AWS JSON protocol puts on the wire; the host builds, signs and dispatches the
//!   request ([`super::aws_api`]) and the answer's document is what the method returns,
//!   with the `ResponseMetadata` the answer itself carried. WHICH operation of the called
//!   service that name is, and how the service itself spells it, is the called service's own
//!   MODEL's to say ([`super::sdk_model::operation_named`]) — a name carrying an acronym
//!   (`GetOTelEnrichment`) is not recoverable from a method name word by word — and a method
//!   naming no operation that model declares does not exist on the client at all, which is
//!   the missing attribute an SDK reports for it.
//! * A refusal is raised as `botocore.exceptions.ClientError`, carrying the service's own
//!   error code and message under the `response` member and rendering the sentence botocore
//!   renders, so a handler that catches one and reports `response["Error"]["Code"]` reads
//!   the emulated service's own answer. Beside them stand the MEMBERS the refusal's own error
//!   shape declares — an error shape is a structure like any other, so a handler reads
//!   `response["CancellationReasons"]`, or whichever members its model gives that class, the
//!   way it reads an answered call's own members. For a service whose model carries the
//!   `awsQueryCompatible` modifier, this SDK's model keys the class by the LEGACY query code
//!   the answer carries for it, so that code is what the refusal reports.
//! * `client.exceptions.<ShapeName>` is the modelled error class for that name, a subclass
//!   of `ClientError`, so `except client.exceptions.QueueDoesNotExist` catches exactly that
//!   refusal and nothing else — the class for the legacy code of that same class and the
//!   class named after the shape are ONE ([`Client`]'s exception table). A code the service's
//!   model declares no error shape for — the AWS-common refusals, which no service's model
//!   carries — has no class of its own in the SDK, so such a refusal is raised as the base
//!   `ClientError` itself ([`error_shapes`]).
//! * WHICH of those classes one call can be raised as is the CALLED OPERATION's own error
//!   list: a refusal is raised as the modelled class for the error that operation declares
//!   under the code the service answered, and as the base `ClientError` for every other code
//!   — an authorization denial, a throttle, any refusal the operation models no shape for —
//!   because an operation is answered with the errors it declares and the ones its service
//!   declares for all of them, and nothing else ([`error_shapes`]).
//! * DynamoDB's RESOURCE interface is supplied too ([`dynamodb`]), because it is the
//!   interface an application stores from inside a function through:
//!   `boto3.resource("dynamodb").Table(name)` carries the table name, its items are plain
//!   Python values rather than the wire's attribute values, and a `Key`/`Attr` condition is
//!   built into the expression the service reads. It is the same client underneath, so the
//!   request, the signature and the refusal are the ones above. A service this runtime
//!   supplies no resource classes for is REFUSED rather than served an interface that does
//!   nothing.
//! * The HTTP CLIENT the SDK is built on is supplied too ([`urllib3`]), because a managed
//!   runtime carries it on the import path beside the SDK that brings it: a handler that
//!   sends a request with `urllib3.PoolManager()` — which is how AWS's own documented
//!   custom-resource response code answers CloudFormation — reaches this emulator over the
//!   same seam the standard library's own client rides.
//! * The SDK is an INSTALLATION rather than two packages, so the distributions that
//!   installation requires are on the import path beside it — the HTTP client above and the
//!   managed transfer library the SDK's own S3 customizations import ([`s3transfer`]) and the
//!   query language its data-driven interfaces are written over ([`jmespath`]) and the date and
//!   time library every timestamp of the wire is read with ([`dateutil`]). A
//!   managed runtime preinstalls all of them, which is what lets a deployment package bundle
//!   the SDK itself and still import: the package's own copy of `boto3` or `botocore` wins
//!   over the runtime's, while a dependency it did not bundle resolves from the runtime behind
//!   it.
//! * The SDK reports the GENERATION it implements, under the names the SDK reports its own
//!   version under (`boto3.__version__`, `botocore.__version__`), because a handler that logs
//!   or branches on the SDK it was given reads them. No release of the published SDK is here
//!   to name, so what they report is the generation and nothing finer.
//! * A state one of the service's resources reaches has the waiter the SDK's own client
//!   answers with: `client.get_waiter(...)` polls the operation that state's own waiter
//!   definition names until an acceptor of it decides the wait ([`waiter`]), which is how a
//!   handler that has just asked for a resource waits for it to be usable; a state the service
//!   publishes no waiter for has none, which is what `waiter_names` reports and the
//!   `ValueError` a lookup of it raises says.
//! * A paginated operation has the paginator the SDK's own client answers with:
//!   `client.get_paginator(...)` pages it through by driving the members its model states
//!   ([`paginator`]), and an operation the model does not mark as paginated has none, which
//!   is what `can_paginate` reports and `OperationNotPageableError` says. A blob member
//!   travels as the base64 text an AWS JSON protocol carries it
//!   as, in both directions, and `bytes` a handler passes are encoded to it. A `datetime`
//!   a handler passes for a timestamp member travels as the seconds since the epoch that
//!   same protocol carries a timestamp as, whether or not it carries a zone of its own. A
//!   member of an ANSWER the called service's model types as a timestamp is handed back the
//!   other way: the SDK materialises the epoch seconds the protocol carried into a `datetime`
//!   in UTC, because that is what a client gives a caller for one, so a handler formats,
//!   compares and does date arithmetic on it. WHICH of
//!   an answer's values those are is the called service's own model's to say
//!   ([`super::blob_members`]), read by the host beside the blob members.
//! * A client is built with the CONFIGURATION the SDK's own `botocore.config.Config`
//!   carries, whose options and defaults are the SDK's: the Region it names is the one the
//!   client's calls are made in, and an option name the SDK does not have is refused the way
//!   the SDK refuses it.
//! * A client built with CREDENTIALS of its own — `boto3.client(…, aws_access_key_id=…,
//!   aws_secret_access_key=…, aws_session_token=…)`, or a `boto3.Session` built with them —
//!   has its calls signed with those, which is how a handler calls a service with a session
//!   it was vended; a client built with neither the key nor its secret is signed as the
//!   identity the invocation was given. Part of that pair is not a credential, and is refused
//!   as the SDK refuses it (`PartialCredentialsError`). The signature itself is still host
//!   code's ([`super::aws_api`]): what the guest hands over is the key it holds, never the
//!   invocation's own.
//! * `session.get_credentials()` resolves the SDK's DEFAULT credential chain for a session
//!   built with no credentials of its own, which in a Lambda execution environment is the
//!   three reserved credential variables the runtime exported — the TEMPORARY credentials of
//!   the session of the function's execution role, session token included
//!   ([`BOTOCORE_CREDENTIALS_SOURCE`]). That is the credential the SDK signs with, so what a
//!   handler reads back is the identity its own calls speak for; an environment holding none
//!   resolves to no credential at all, which is what the SDK's own chain answers then.
//! * A member of an ANSWER that carries an object's own bytes — `GetObject`'s `Body` — is
//!   handed over as `botocore.response.StreamingBody`, the stream the SDK's own client
//!   answers with, so a handler reads it with `read()`; which members those are is named by
//!   the answer itself ([`super::aws_api::Answer::streams`]).
//! * Nothing here is the containment. The services that can be reached, the bounds on a
//!   call and the signature it carries are all host code's ([`super::aws_api`],
//!   [`super::python`]); a handler that rebinds or deletes a name in these modules changes
//!   none of them.

use super::package::PackageModule;


/// DynamoDB's resource interface: the service resource, its tables, and the item
/// marshalling between a plain Python value and an attribute value.
mod dynamodb;

/// The paginator a client answers with for a paginated operation.
mod paginator;

/// The waiter a client answers with for a state one of the service's resources reaches.
mod waiter;

/// The HTTP client the SDK is built on, which a managed runtime ships beside it.
mod urllib3;

/// The managed transfer library the SDK's own S3 customizations are built on, which the same
/// installation carries.
mod s3transfer;

/// The query language the SDK's own data-driven interfaces are written over, which the same
/// installation carries.
mod jmespath;

/// The date and time library every timestamp of the wire is read with, which the same
/// installation carries.
mod dateutil;

/// The modules this runtime puts on a Python handler's import path, as the SDK it ships.
///
/// Resolved AFTER the deployment package's own entries and after the content the function's
/// LAYERS mount, so a package — or a layer — that ships its own `boto3` wins: that is the
/// order a real runtime resolves them in, where the task root and the layer root's own
/// directories precede the runtime's.
#[must_use]
pub fn runtime_modules() -> Vec<PackageModule> {
    [
        ("botocore/__init__.py", BOTOCORE_SOURCE),
        ("botocore/exceptions.py", BOTOCORE_EXCEPTIONS_SOURCE),
        ("botocore/config.py", BOTOCORE_CONFIG_SOURCE),
        ("botocore/response.py", BOTOCORE_RESPONSE_SOURCE),
        ("botocore/credentials.py", BOTOCORE_CREDENTIALS_SOURCE),
        ("botocore/awsrequest.py", BOTOCORE_AWSREQUEST_SOURCE),
        ("botocore/auth.py", BOTOCORE_AUTH_SOURCE),
        ("botocore/session.py", BOTOCORE_SESSION_SOURCE),
        ("botocore/client.py", BOTOCORE_CLIENT_SOURCE),
        ("boto3/__init__.py", BOTO3_SOURCE),
        ("boto3/session.py", BOTO3_SESSION_SOURCE),
    ]
    .into_iter()
    .chain(paginator::modules())
    .chain(waiter::modules())
    .chain(dynamodb::modules())
    .chain(urllib3::modules())
    .chain(s3transfer::modules())
    .chain(jmespath::modules())
    .chain(dateutil::modules())
    .map(|(path, source)| PackageModule {
        path: path.to_string(),
        source: source.to_string(),
    })
    .collect()
}

/// What the paginator of one operation of the service `service` names drives, as the document
/// a client reads it from — an empty one for an operation that service's model does not mark
/// as paginated, and so has no paginator for.
///
/// Every member of it is the called operation's own model's ([`super::sdk_model`]), read from
/// the same reading of that model every other question about the service is answered from, so
/// the paginator a handler is given drives the members the service really pages that operation
/// through.
#[must_use]
pub fn pagination_config(service: &str, operation: &str) -> String {
    let operation = modelled_operation(service, operation);
    let held = super::sdk_model::client_exports(service);
    let Some(paged) = held.pagination.get(operation.as_str()) else {
        return "{}".to_string();
    };
    ::serde_json::json!({
        "input_token": paged.input_token,
        "output_token": paged.output_token,
        "limit_key": paged.page_size,
        "result_key": paged.result_keys,
    })
    .to_string()
}

/// The error SHAPES the service `service` declares, as the list a client reads its own
/// exception classes off — or, for `Some(operation)`, the shapes THAT operation can be answered
/// with. Empty for a service this emulator holds no model for, and for an operation it holds no
/// model of.
///
/// An SDK has an exception class for each error its service's model declares and for nothing
/// else, so the service's list is what decides whether a name has a class at all: a code the
/// model declares no error shape for — the AWS-common refusals every service answers, which no
/// service's model carries — has no class in the SDK and is handed over as the base
/// [`ClientError`](BOTOCORE_EXCEPTIONS_SOURCE).
///
/// WHICH of those classes one answered call can be is narrower, and that is what the
/// `operation` list answers: a call is answered with the errors the operation declares plus the
/// ones its service declares for every operation, so a code outside that list is a refusal the
/// operation models no shape for and is raised as the base class however many other operations
/// of the service do model it.
///
/// Read from the same reading of that model every other question about the service is answered
/// from ([`super::sdk_model`]), so the classes this SDK has are the ones the service really
/// declares.
#[must_use]
pub fn error_shapes(service: &str, operation: Option<&str>) -> String {
    let held = super::sdk_model::client_exports(service);
    let Some(operation) = operation else {
        return ::serde_json::json!(held.errors).to_string();
    };
    let operation = modelled_operation(service, operation);
    ::serde_json::json!(held
        .operation_errors
        .get(operation.as_str())
        .cloned()
        .unwrap_or_default())
    .to_string()
}

/// The operation `asked` names, under the spelling the service `service` names states for it
/// ([`super::sdk_model::operation_named`]).
///
/// Every question answered out of a called service's model is a question about ONE of its
/// operations, and the name asking it arrives from an SDK CLIENT, which derived it from the
/// method a handler called: an operation whose name carries an acronym is asked about under
/// that SDK's own casing of the method name rather than under the service's. So the name is
/// resolved to the model's spelling first, and a name that model declares no operation of is
/// asked exactly as it arrived — a question about an operation the service does not have,
/// which its model answers nothing for.
fn modelled_operation(service: &str, asked: &str) -> String {
    super::sdk_model::operation_named(service, asked)
        .modelled()
        .unwrap_or(asked)
        .to_string()
}

/// `botocore`: the package the client interface and the exceptions live under.
///
/// `xform_name` lives here because that is where the SDK keeps it: it is the one rule that
/// turns a model's own member or operation name into the Python one an SDK reports it under
/// (`TableName` -> `table_name`, `LatestStreamARN` -> `latest_stream_arn`).
///
/// `search_path` lives here for the same reason: reading the value a path of member names
/// reaches in one answer is what the SDK's own data-driven interfaces are written over — the
/// paginator's continuation and result members ([`paginator`]) and a waiter's acceptors
/// ([`waiter`]) — so the two drive ONE reader rather than a reading each.
const BOTOCORE_SOURCE: &str = r#"
"""The AWS SDK for Python's lower half, as this runtime supplies it."""

import re

# The GENERATION of the SDK these modules implement, which is the one a managed `python3.x`
# runtime ships: a handler reads it to report the SDK it was given. It is not a published
# release of the SDK — no release of one is here to name — so it names the generation and
# nothing finer.
__version__ = "1.0.0"

_first_cap_regex = re.compile("(.)([A-Z][a-z]+)")
_number_cap_regex = re.compile("([a-z])([0-9]+)")
_end_cap_regex = re.compile("([a-z0-9])([A-Z])")
# A pluralised acronym (`ARNs`, `ACLs`, `CNAMEs`) is one word, not one per letter.
_special_case_transform = re.compile("[A-Z]{2,}s$")


def xform_name(name, sep="_"):
    """A model's name as the Python one the SDK reports it under."""
    if sep in name:
        # Already transformed.
        return name
    special = _special_case_transform.search(name)
    if special is not None:
        matched = special.group()
        name = "%s%s%ss" % (name[: -len(matched)], sep, matched[:-1].lower())
    first = _first_cap_regex.sub(r"\1" + sep + r"\2", name)
    numbered = _number_cap_regex.sub(r"\1" + sep + r"\2", first)
    return _end_cap_regex.sub(r"\1" + sep + r"\2", numbered).lower()


def _resolve(held, steps):
    """The value the remaining `steps` of a path reach in `held`."""
    if not steps:
        return held
    step = steps[0]
    rest = steps[1:]
    each = step.endswith("[]")
    name = step[:-2] if each else step
    if name:
        if not isinstance(held, dict):
            return None
        held = held.get(name)
    if not each:
        return _resolve(held, rest)
    if not isinstance(held, list):
        return None
    return [_resolve(element, rest) for element in held]


def search_path(document, expression):
    """The value `expression` names in `document`, or `None` where it holds none.

    An expression is the member names from the root of an answer, separated by dots, so a
    member of a nested structure is reached the way the model states it; `[]` after a list
    member means every element of it, which is how the SDK's own data names a member of each
    structure of a list. A path the answer does not carry reaches nothing.
    """
    return _resolve(document, [step for step in str(expression).split(".") if step])

"#;

/// `botocore.exceptions`: the exception types a handler catches.
///
/// `ClientError`'s sentence is the one botocore renders — "An error occurred (<Code>) when
/// calling the <Operation> operation: <Message>" — because that sentence is what a handler
/// logs and what a caller reading the handler's report sees.
const BOTOCORE_EXCEPTIONS_SOURCE: &str = r#"
"""The exceptions an AWS SDK call raises."""


class BotoCoreError(Exception):
    """A failure of the SDK itself rather than an answer from a service."""

    fmt = "An unspecified error occurred"

    def __init__(self, **kwargs):
        Exception.__init__(self, self.fmt.format(**kwargs))
        self.kwargs = kwargs


class DataNotFoundError(BotoCoreError):
    fmt = "Unable to load data for: {data_path}"


class UnknownServiceError(DataNotFoundError):
    fmt = "Unknown service: '{service_name}'. Valid service names are: {known_service_names}"


class ParamValidationError(BotoCoreError):
    fmt = "Parameter validation failed:\n{report}"


class PartialCredentialsError(BotoCoreError):
    """A client was built with part of a credential, which is not a credential at all."""

    fmt = "Partial credentials found in {provider}, missing: {cred_var}"


class PaginationError(BotoCoreError):
    fmt = "Error during pagination: {message}"


class OperationNotPageableError(BotoCoreError):
    fmt = "Operation cannot be paginated: {operation_name}"


class WaiterError(BotoCoreError):
    """A wait ended without the state it was waiting for, and why.

    `last_response` is the answer the wait ended on, which is where a handler reads what the
    resource's state really was.
    """

    fmt = "Waiter {name} failed: {reason}"

    def __init__(self, name, reason, last_response):
        BotoCoreError.__init__(self, name=name, reason=reason)
        self.last_response = last_response


class ReadTimeoutError(BotoCoreError):
    fmt = 'Read timeout on endpoint URL: "{endpoint_url}"'


class SdkCallNotMade(BotoCoreError):
    """The call could not be made at all, and why. Carries no answer from any service."""

    fmt = "{detail}"


class ClientError(Exception):
    """A service answered the call with a refusal."""

    MSG_TEMPLATE = (
        "An error occurred ({error_code}) when calling the {operation_name} "
        "operation: {error_message}"
    )

    def __init__(self, error_response, operation_name):
        error = error_response.get("Error", {})
        Exception.__init__(
            self,
            self.MSG_TEMPLATE.format(
                error_code=error.get("Code", "Unknown"),
                error_message=error.get("Message", "Unknown"),
                operation_name=operation_name,
            ),
        )
        self.response = error_response
        self.operation_name = operation_name


class InvalidRetryConfigurationError(BotoCoreError):
    fmt = (
        'Cannot provide retry configuration for "{retry_config_option}". '
        "Valid retry configuration options are: {valid_options}"
    )


class InvalidMaxRetryAttemptsError(InvalidRetryConfigurationError):
    fmt = (
        'Value provided to "max_attempts": {provided_max_attempts} must be an integer '
        "greater than or equal to {min_value}."
    )


class InvalidRetryModeError(InvalidRetryConfigurationError):
    fmt = (
        'Invalid value provided to "mode": "{provided_retry_mode}" must be one of: '
        "{valid_modes}"
    )
"#;

/// `botocore.config`: the client configuration a handler builds a client with.
///
/// The managed runtimes' SDK carries this module, and a handler that wants a call made
/// differently from the default — a region of its own, no retry at all, a timeout — passes
/// `botocore.config.Config(...)` to the client it builds. What it holds is the SDK's own
/// option set, in the SDK's own order, so a value passed positionally lands on the option
/// botocore binds it to and an option name the SDK does not have is refused the way the SDK
/// refuses it. Each option's default is the SDK's own.
///
/// The CALL itself is the host's ([`super::aws_api`]), which makes one attempt into this
/// process over an in-memory seam: there is no connection to time out and no attempt to
/// repeat, so the options bearing on transport are carried and reported rather than acted
/// on. The one option that decides where a call goes — the region — IS acted on by the
/// client built with it.
const BOTOCORE_CONFIG_SOURCE: &str = r#"
"""The configuration one client is built with."""

from botocore.exceptions import (
    InvalidMaxRetryAttemptsError,
    InvalidRetryConfigurationError,
    InvalidRetryModeError,
)

# The retry members a `retries` configuration may carry, and the modes it may name
# (`Config(retries={"max_attempts": 3, "mode": "standard"})`).
_RETRY_OPTIONS = ("total_max_attempts", "max_attempts", "mode")
_RETRY_MODES = ("legacy", "standard", "adaptive")


class Config:
    """A client's configuration, as `botocore.config.Config` holds one.

    The options are the SDK's own, in the SDK's own order: a positional argument lands on
    the option that order binds it to, and a name this set does not hold is a `TypeError`.
    """

    OPTION_DEFAULTS = {
        "region_name": None,
        "signature_version": None,
        "user_agent": None,
        "user_agent_extra": None,
        "user_agent_appid": None,
        "connect_timeout": 60,
        "read_timeout": 60,
        "parameter_validation": True,
        "max_pool_connections": 10,
        "proxies": None,
        "proxies_config": None,
        "s3": None,
        "retries": None,
        "client_cert": None,
        "inject_host_prefix": True,
        "endpoint_discovery_enabled": None,
        "use_dualstack_endpoint": None,
        "use_fips_endpoint": None,
        "ignore_configured_endpoint_urls": None,
        "defaults_mode": None,
        "tcp_keepalive": None,
        "request_min_compression_size_bytes": None,
        "disable_request_compression": None,
        "client_context_params": None,
        "sigv4a_signing_region_set": None,
        "request_checksum_calculation": None,
        "response_checksum_validation": None,
        "account_id_endpoint_mode": None,
    }

    def __init__(self, *args, **kwargs):
        self._user_provided_options = self._record_user_provided_options(args, kwargs)
        options = dict(self.OPTION_DEFAULTS)
        options.update(self._user_provided_options)
        for option, value in options.items():
            setattr(self, option, value)
        self._validate_retry_configuration(self.retries)

    def _record_user_provided_options(self, args, kwargs):
        option_order = list(self.OPTION_DEFAULTS)
        provided = {}
        for key, value in kwargs.items():
            if key not in self.OPTION_DEFAULTS:
                raise TypeError("Got unexpected keyword argument %r" % (key,))
            provided[key] = value
        if len(args) > len(option_order):
            raise TypeError(
                "Takes at most %s arguments (%s given)" % (len(option_order), len(args))
            )
        for index, argument in enumerate(args):
            option = option_order[index]
            if option in provided:
                raise TypeError("Got multiple values for keyword argument %r" % (option,))
            provided[option] = argument
        return provided

    def _validate_retry_configuration(self, retries):
        if not retries:
            return
        for key, value in retries.items():
            if key not in _RETRY_OPTIONS:
                raise InvalidRetryConfigurationError(
                    retry_config_option=key,
                    valid_options=", ".join(_RETRY_OPTIONS),
                )
            if key == "max_attempts" and value < 0:
                raise InvalidMaxRetryAttemptsError(
                    provided_max_attempts=value, min_value=0
                )
            if key == "total_max_attempts" and value < 1:
                raise InvalidMaxRetryAttemptsError(
                    provided_max_attempts=value, min_value=1
                )
            if key == "mode" and value not in _RETRY_MODES:
                raise InvalidRetryModeError(
                    provided_retry_mode=value, valid_modes=", ".join(_RETRY_MODES)
                )

    def merge(self, other_config):
        """This configuration with `other_config`'s own options over it.

        "Any options not provided by the user in the other config object will be the
        options this object holds", so what the caller of either one set wins in the order
        they are merged, and an option neither set keeps its default.
        """
        options = dict(self._user_provided_options)
        options.update(other_config._user_provided_options)
        return Config(**options)
"#;

/// `botocore.response`: the body of an answer that carries BYTES rather than text.
///
/// An object read answers with the object's own payload, which the SDK's client hands over
/// as a stream (`GetObject`'s `Body`), so a handler reads it with `read()`, iterates its
/// lines or uses it as a context manager. The bytes are already here — the whole answer
/// crossed the runtime's own call seam before the client returned — so no read of one waits
/// on anything.
const BOTOCORE_RESPONSE_SOURCE: &str = r#"
"""The body of an answer carrying bytes, as this runtime's SDK hands one over."""


class StreamingBody:
    """The bytes one answer carried, read as the stream a client answers with."""

    def __init__(self, data, content_length=None):
        self._data = bytes(data)
        self._position = 0
        self.content_length = (
            len(self._data) if content_length is None else content_length
        )

    def read(self, amt=None):
        """The next `amt` bytes, or everything that is left."""
        if amt is None:
            chunk = self._data[self._position:]
        else:
            chunk = self._data[self._position:self._position + max(int(amt), 0)]
        self._position += len(chunk)
        return chunk

    def close(self):
        self._position = len(self._data)

    def tell(self):
        return self._position

    def iter_chunks(self, chunk_size=1024):
        while True:
            chunk = self.read(chunk_size)
            if not chunk:
                return
            yield chunk

    def iter_lines(self, chunk_size=1024, keepends=False):
        for line in self.read().splitlines(keepends):
            yield line

    def __iter__(self):
        return self.iter_lines()

    def __enter__(self):
        return self

    def __exit__(self, *_exception):
        self.close()
        return False
"#;

/// `botocore.credentials`: the credentials the SDK's DEFAULT chain resolves inside a run.
///
/// An SDK's default credential chain reads the credentials of the environment it runs in, and
/// in a Lambda execution environment those are the three reserved variables the runtime
/// exported for the session of the function's execution role ([`super::runtime_environment`]).
/// So a handler that asks the session for its credentials — to read its own identity, or to
/// hand them to [`BOTOCORE_AUTH_SOURCE`]'s signer — is answered with exactly those three
/// values, and an environment holding none resolves to no credential at all rather than to an
/// empty one.
///
/// The signature of a CALL is still put on by host code ([`super::aws_api`]): what the chain
/// answers here is the credential the run was given, not a second one minted for the guest.
const BOTOCORE_CREDENTIALS_SOURCE: &str = r#"
"""The credentials the SDK's default chain resolves for this invocation."""

# The provider name a credential resolved from the environment carries, and the one a
# credential handed to the client itself carries.
_ENVIRONMENT_METHOD = "env"
_EXPLICIT_METHOD = "explicit"

# The three reserved variables a managed runtime exports the invocation's own credentials in
# (the Lambda Developer Guide's reserved runtime environment variables), which is where the
# default chain reads them.
_ACCESS_KEY_VARIABLE = "AWS_ACCESS_KEY_ID"
_SECRET_KEY_VARIABLE = "AWS_SECRET_ACCESS_KEY"
_SESSION_TOKEN_VARIABLE = "AWS_SESSION_TOKEN"


class Credentials:
    """One credential the SDK signs with: an access key, its secret and a session token.

    A credential resolved from the environment is the TEMPORARY credential of the session of
    the function's execution role, so it carries a session token; one a handler built itself
    carries whatever it was given. The secret is never used to sign in this runtime — the host
    signs every call (see `botocore.auth`) — so what is held here is only what the run was
    already given in its own environment.
    """

    def __init__(self, access_key=None, secret_key=None, token=None, method=None):
        self.access_key = access_key
        self.secret_key = secret_key
        self.token = token
        self.method = _EXPLICIT_METHOD if method is None else method

    def get_frozen_credentials(self):
        return self


def resolve_environment_credentials():
    """What the DEFAULT credential chain resolves in this execution environment.

    The run's own environment, which is all a handler has: the three reserved variables the
    runtime exported. `None` where it holds no credential — a key without its secret is not
    one — which is what the chain answers when nothing resolves.
    """
    import os

    access_key = os.environ.get(_ACCESS_KEY_VARIABLE)
    secret_key = os.environ.get(_SECRET_KEY_VARIABLE)
    if not access_key or not secret_key:
        return None
    return Credentials(
        access_key=access_key,
        secret_key=secret_key,
        token=os.environ.get(_SESSION_TOKEN_VARIABLE) or None,
        method=_ENVIRONMENT_METHOD,
    )
"#;

/// `botocore.awsrequest`: the request a handler builds to sign and send itself.
const BOTOCORE_AWSREQUEST_SOURCE: &str = r#"
"""One HTTP request a handler builds for itself."""


class AWSRequest:
    """A request a handler signs and then sends with an HTTP client of its own.

    The members are botocore's: `method`, `url`, `headers`, `body` (which `data` is the
    other name of), and `params` for the query. Nothing is sent by building one.
    """

    def __init__(self, method=None, url=None, headers=None, data=None, params=None):
        self.method = method
        self.url = url
        self.headers = dict(headers or {})
        self.data = data
        self.params = dict(params or {})
        self.context = {}

    @property
    def body(self):
        return self.data

    @body.setter
    def body(self, value):
        self.data = value

    def prepare(self):
        return self
"#;

/// `botocore.auth`: the signer a handler puts on a request of its own.
///
/// WHAT IT DOES: it records on the request which service and Region the signature is for,
/// and nothing else. The signature itself is put on by HOST code when the request is sent
/// ([`super::guest_http`]), with a credential the guest never holds — so this writes no
/// `Authorization` header, because it has nothing to write one from. A handler that signs a
/// request and sends it with an HTTP client reaches the service it addressed, signed; a
/// handler that reads the header back finds the request as it left it.
const BOTOCORE_AUTH_SOURCE: &str = r#"
"""Signing one request a handler built itself."""

_SIGNING_CONTEXT = "emulator_signing"


class BaseSigner:
    def add_auth(self, request):
        raise NotImplementedError("add_auth")


class SigV4Auth(BaseSigner):
    """The SigV4 signer, as a handler builds one: a credential handle, a service and a
    Region.

    `add_auth` states on the request WHICH service and Region its signature is for. The
    signature is put on by the runtime when the request is sent, out of a credential the
    handler never holds, so nothing secret passes through here and no signature is
    invented.
    """

    def __init__(self, credentials, service_name, region_name):
        self.credentials = credentials
        self.service_name = service_name
        self.region_name = region_name

    def add_auth(self, request):
        context = getattr(request, "context", None)
        if context is None:
            context = {}
            request.context = context
        context[_SIGNING_CONTEXT] = {
            "service": self.service_name,
            "region": self.region_name,
        }
        return request


class SigV4QueryAuth(SigV4Auth):
    """The same signer for a signature carried in the query, which is what a presigned URL
    carries. A URL is presigned by the client itself
    (`client.generate_presigned_url(...)`), where the runtime signs it; this exists so a
    handler that names the class finds it."""

    def __init__(self, credentials, service_name, region_name, expires=3600):
        SigV4Auth.__init__(self, credentials, service_name, region_name)
        self.expires = expires
"#;

/// `botocore.session`: the session the lower half of the SDK is reached through.
const BOTOCORE_SESSION_SOURCE: &str = r#"
"""The botocore session a handler asks for its credentials or a client."""

from botocore.credentials import resolve_environment_credentials


class Session:
    def __init__(self, **_ignored):
        self._credentials = None

    def get_credentials(self):
        """The credentials this session resolves, as the SDK's default chain resolves them.

        A session built by a handler names none of its own, so what it answers is what the
        execution environment holds: the session of the function's execution role. `None`
        where nothing resolves, which is what the SDK's own chain answers then.
        """
        if self._credentials is None:
            self._credentials = resolve_environment_credentials()
        return self._credentials

    def get_scoped_config(self):
        return {}

    def create_client(self, service_name, region_name=None, **kwargs):
        from botocore.client import Client

        return Client(service_name, region_name=region_name, **kwargs)


def get_session(**kwargs):
    return Session(**kwargs)
"#;

/// `botocore.client`: the client itself, and the one host call behind every method.
const BOTOCORE_CLIENT_SOURCE: &str = r##"
"""One AWS SDK client: a method per operation, over the runtime's own call seam."""

import base64
import json

from botocore import xform_name
from botocore.config import Config
from botocore.exceptions import (
    ClientError,
    OperationNotPageableError,
    ParamValidationError,
    PartialCredentialsError,
    ReadTimeoutError,
    SdkCallNotMade,
    UnknownServiceError,
)
from botocore.response import StreamingBody

_host = __import__("_lambda_host")

# The client that carries `generate_db_auth_token`: the SDK adds it to the RDS client
# rather than to every client, because the token is a database login's, not an API call's.
_DB_AUTH_TOKEN_SERVICES = ("rds",)

# The MANAGED-TRANSFER methods the SDK adds to the Amazon S3 client and to no other, by the
# name a handler calls each of them under: "upload_fileobj", "download_fileobj",
# "upload_file" and "download_file" are documented on that client beside its operations, so
# a handler that moves an object through a buffer or through the function's own ephemeral
# storage calls them rather than an operation. They are the client's and not the service's:
# each is one `PutObject` or `GetObject` made through this same client.
_S3_TRANSFER_SERVICES = ("s3",)
_S3_TRANSFER_METHODS = (
    "upload_fileobj",
    "download_fileobj",
    "upload_file",
    "download_file",
)

# The input members a call carries BYTES in, by service, keyed by the service's token with
# its hyphens dropped — the one spelling both of a two-word token's forms compact to, so a
# client built with either reaches the same entry. Every protocol this SDK speaks carries a
# blob as base64 text, so the bytes a handler passes in one of these are encoded before the
# call is made; a `str` is encoded as UTF-8 first, which is what a client does with a text
# body. The members of the ANSWER that carry bytes are named by the answer itself, so
# nothing about them is listed here.
_BLOB_PARAMETERS = {
    "s3": ("Body",),
    "appconfig": ("Content",),
    "apigatewaymanagementapi": ("Data",),
    # The IoT data plane carries the MQTT message and the shadow state document it is given
    # at one member name, whichever operation is called: `payload`.
    "iotdata": ("payload",),
    "iotdataplane": ("payload",),
}


def _operation_name(method):
    """The operation a client method names: `send_message` -> `SendMessage`.

    WHICH operation of the called service a name is, and how that service spells it, is that
    service's own model's to say: the runtime resolves this name against the model before it
    builds the request, so an operation whose name carries an acronym (`GetOTelEnrichment`) is
    reached under the spelling the service itself gives it rather than under the one a method
    name's words give.
    """
    return "".join(part[:1].upper() + part[1:] for part in method.split("_") if part)


# The provider a partial-credential refusal names, which is the one botocore names for
# credentials given to the client itself rather than read from a profile or the environment.
_EXPLICIT_PROVIDER = "explicit"


def _supplied_credentials(access_key_id, secret_access_key, session_token):
    """The credentials a client was built with, as the runtime signs that client's calls with.

    An SDK signs a call with the credentials its own client holds: a client built with none
    signs as the identity the invocation was given — the session of the function's execution
    role — and one built with an access key and its secret signs as THAT credential, which is
    how a handler calls a service with a session it was vended. The values travel to the
    runtime, which puts the signature on; the empty document is a client that was given none.

    The explicit pair is the KEY and its SECRET: a client given neither falls through to the
    invocation's own credentials whatever else it was passed, and one given a key without its
    secret, or a secret without its key, holds no credential at all — which the SDK says
    rather than signing as something else.
    """
    if access_key_id is None and secret_access_key is None:
        return ""
    missing = [
        name
        for name, value in (
            ("aws_access_key_id", access_key_id),
            ("aws_secret_access_key", secret_access_key),
        )
        if value is None
    ]
    if missing:
        raise PartialCredentialsError(
            provider=_EXPLICIT_PROVIDER, cred_var=", ".join(missing)
        )
    return json.dumps(
        {
            "accessKeyId": str(access_key_id),
            "secretAccessKey": str(secret_access_key),
            "sessionToken": "" if session_token is None else str(session_token),
        }
    )


def _epoch_seconds(value):
    """A `datetime` as the seconds since the epoch a JSON protocol carries a timestamp as.

    An SDK takes a native date-time for an input member the model types as a timestamp and
    sends it in the form the called service's protocol carries one in, which for the AWS
    JSON protocols this SDK speaks is the seconds since the epoch. A value carrying its own
    zone is read in that zone; one carrying none is read as UTC, the zone a function runs
    in, so the two forms of the same instant travel as the same number.
    """
    import datetime

    if value.tzinfo is not None:
        value = value.astimezone(datetime.timezone.utc).replace(tzinfo=None)
    return int((value - datetime.datetime(1970, 1, 1)).total_seconds())


def _file_like_bytes(value):
    """The bytes a FILE-LIKE value carries, read from where it stands, or `None`.

    An SDK takes a blob input member as "bytes or seekable file-like object" (Amazon S3
    `PutObject$Body` is documented in exactly those words), so a buffer a handler built in
    memory and a file it opened are both values a call carries. It is read from the position
    the caller left it at, which is what the SDK reads one from: a buffer written and then
    rewound carries all of it, and one that was not carries what is left of it. Text a
    file-like value in text mode hands back is encoded as UTF-8, the way a text body is.

    `None` for a value that is not file-like at all, which leaves the refusal to the caller.
    A value that IS file-like and whose `read` hands back neither text nor bytes answers
    `None` too, with that read already spent: there is no way to put back what a stream
    yielded, so such a value is unrecoverable and the caller's refusal reports the value
    rather than its contents.
    """
    read = getattr(value, "read", None)
    if not callable(read):
        return None
    carried = read()
    if isinstance(carried, str):
        return carried.encode("utf-8")
    if isinstance(carried, (bytes, bytearray)):
        return bytes(carried)
    return None


def _blob_bytes(value):
    """The bytes a blob input member carries, in each of the forms an SDK call takes one.

    The three the SDK documents: the `bytes` themselves, text (encoded as UTF-8, which is
    what a client does with a text body), and a file-like object ([`_file_like_bytes`]).
    `None` for anything else — the value the caller then refuses ([`_blob_type_report`]).
    """
    if isinstance(value, (bytes, bytearray)):
        return bytes(value)
    if isinstance(value, str):
        return value.encode("utf-8")
    return _file_like_bytes(value)


def _blob_type_report(member, value):
    """The report a blob member given a value no call carries is refused with.

    The SDK validates the parameter before it builds the request and names the member, what
    it was given and the forms it takes. The value is rendered AS IT STANDS, the way the
    SDK's own report renders it: `BotoCoreError` substitutes this report into its own
    sentence with `str.format`, which parses only the sentence's own braces and emits a
    substituted value verbatim, so a value whose rendering carries braces reads exactly as
    the value does.
    """
    return (
        "Invalid type for parameter %s, value: %s, type: %s, valid types: "
        "<class 'bytes'>, <class 'bytearray'>, file-like object" % (member, value, type(value))
    )


def _encoded(value):
    """A value `json` does not write by itself.

    Every protocol this SDK speaks carries a blob as base64 text, so the `bytes` a handler
    passes are encoded to it — as are the bytes a SEEKABLE file-like value carries, which is
    the form an SDK takes a blob in ([`_file_like_bytes`]) — and a timestamp as the seconds
    since the epoch, so a `datetime` a handler passes for a timestamp member is converted to
    them ([`_epoch_seconds`]) rather than refused: a native date-time is what an SDK takes
    for such a member. Anything else is a value this SDK cannot send, and says so.
    """
    import datetime

    if isinstance(value, (bytes, bytearray)):
        return base64.b64encode(bytes(value)).decode("ascii")
    if isinstance(value, datetime.datetime):
        return _epoch_seconds(value)
    # A blob input member takes "a seekable file-like object", so the arm offered here is
    # narrowed to one: this hook sees EVERY value a call's document carries, not only the
    # members a model types as a blob, and a value that merely happens to carry a `read` is
    # not one of them.
    if callable(getattr(value, "read", None)) and callable(getattr(value, "seek", None)):
        carried = _file_like_bytes(value)
        if carried is not None:
            return base64.b64encode(carried).decode("ascii")
    raise TypeError("%r is not a value an AWS SDK call can carry" % (value,))


def _copy_source(value):
    """The `CopySource` of a copy, as the SDK renders one.

    `CopySource` travels as the text of a header, and the SDK lets a caller give it either
    as that text or as `{"Bucket": …, "Key": …, "VersionId": …}` — in which case the SDK is
    what renders it: the bucket, a slash, the key with the characters a header cannot carry
    percent-encoded, and the version the copy reads, when one is named.
    """
    if not isinstance(value, dict):
        return value
    from urllib.parse import quote

    rendered = "%s/%s" % (
        value.get("Bucket", ""),
        quote(str(value.get("Key", "")).encode("utf-8"), safe="/~"),
    )
    version = value.get("VersionId")
    if version:
        rendered += "?versionId=%s" % (quote(str(version).encode("utf-8"), safe="/~"),)
    return rendered


def _materialise(document, path, convert):
    """Replace the value `path` reaches with what `convert` makes of it.

    `path` is the object keys and list indices that reach one value from the root of the
    answer's document, as the runtime read them out of the called operation's own model. A
    path the document does not hold, and a value `convert` answers nothing for, are left
    exactly as they arrived.
    """
    if not path:
        return
    held = document
    for step in path[:-1]:
        try:
            held = held[step]
        except (KeyError, IndexError, TypeError):
            return
    last = path[-1]
    try:
        carried = held[last]
    except (KeyError, IndexError, TypeError):
        return
    replaced = convert(carried)
    if replaced is not None:
        held[last] = replaced


def _bytes_of(carried):
    """The BYTES the base64 text of a blob carries, which is what the SDK gives a caller for a
    blob member: a JSON protocol has no byte type, so one travels as that text. A value that is
    not text is left as it arrived.
    """
    if isinstance(carried, str):
        return base64.b64decode(carried)
    return None


def _instant_of(carried):
    """The native `datetime` an SDK gives a caller for a member its service's model types as a
    timestamp, which is what a handler reads `.isoformat()`, `.timestamp()` and the arithmetic
    of a date off.

    A JSON protocol carries a timestamp as the seconds since the epoch — whole or fractional —
    and the SDK reports one in UTC, as this SDK's own date and time library spells that zone;
    the fraction is kept to the microsecond a `datetime` holds. A member whose own trait states
    the ISO text instead carries that text, which is parsed the way the SDK parses it. A value
    in neither form — and a boolean, which is not a number here however Python types it — is
    left exactly as it arrived rather than reported as an instant nothing answered. So is a
    number no instant can be built from: an epoch beyond what a `datetime` holds is reported as
    the number the answer carried rather than as a failure of the call that answered it.
    """
    import datetime

    from dateutil.tz import tzutc

    if isinstance(carried, bool):
        return None
    if isinstance(carried, (int, float)):
        try:
            return datetime.datetime(1970, 1, 1, tzinfo=tzutc()) + datetime.timedelta(
                microseconds=round(carried * 1000000)
            )
        except (ValueError, OverflowError, OSError):
            return None
    if isinstance(carried, str):
        from dateutil.parser import parse

        try:
            return parse(carried)
        except (TypeError, ValueError, OverflowError):
            return None
    return None


def _error_shape(error_type, document):
    """The modelled error SHAPE a refusal names, from the header or the document's own member."""
    for named in (
        error_type,
        document.get("__type"),
        document.get("Code"),
        document.get("code"),
    ):
        if not named:
            continue
        shape = named.split("#")[-1].split(":")[0].strip()
        if shape:
            return shape
    return ""


def _error_code(query_code, shape):
    """The error code a refusal names, as this SDK reports it.

    A query-compatible service kept the error vocabulary of the query protocol it moved off,
    and this SDK's model keys such a service's error classes by that LEGACY code, so the code
    it reports for the class is the one the answer carried beside the shape. Every other
    service has only the shape, which is then the code.
    """
    return query_code or shape or "Unknown"


def _error_message(document):
    """The message a refusal carries, as the SDK reports it on `Error.Message`.

    The member as the answer carried it, where that is a value a message can be: the TEXT of
    a string, and `None` for a service that answers a refusal whose `message` is null, which
    is what the SDK renders the sentence of such a refusal from. Anything else — a member
    holding a document or a number, which no refusal of this wire carries — is reported the
    way a document carrying no message at all is, because `Error.Message` is a string.
    """
    for member in ("message", "Message", "errorMessage"):
        if member not in document:
            continue
        held = document[member]
        if held is None or isinstance(held, str):
            return held
    return ""


def _member_of(document, carried):
    """The value `document` holds for the member the wire carries under `carried`.

    Answers the value and whether the document carried the member at all, so a member it
    carried as null is reported as the null the service answered rather than as absent. The key
    is the member's own on the wire; a protocol that spells one in a different CASE — the
    message of an AWS JSON refusal, which travels as `message` — carries the same member, so a
    key differing from it only in case answers too.
    """
    if carried in document:
        return document[carried], True
    folded = str(carried).lower()
    for held in document:
        if isinstance(held, str) and held.lower() == folded:
            return document[held], True
    return None, False


class _ClientExceptions:
    """`client.exceptions`: the modelled error class for each error code."""

    def __init__(self, service_name):
        self._service_name = service_name
        self._classes = {"ClientError": ClientError}
        self._declared = {}

    def __getattr__(self, name):
        if name.startswith("_"):
            raise AttributeError(name)
        return self.from_code(name)

    def _modelled(self, operation=None):
        """The error shapes DECLARED: the called service's own, or one operation's.

        An SDK carries one exception class per error its service declares, so the service's
        list is what says whether a name has a class of its own at all; an OPERATION's list is
        what says which of those classes a call of it can be answered with. Read once per
        client and per operation asked about, from the service's own model.
        """
        key = operation or ""
        held = self._declared.get(key)
        if held is None:
            held = frozenset(json.loads(_host.error_shapes(self._service_name, key)))
            self._declared[key] = held
        return held

    def from_code(self, code, shape=None, operation=None):
        """The class for one error code, which is the class the modelled SHAPE names too.

        This SDK's model keys a query-compatible service's error class by the LEGACY code,
        so the class named after the modelled shape and the class for that code are ONE:
        `except client.exceptions.<Shape>` catches the refusal whose reported code is the
        legacy one. `shape` is the shape the answer named, which is what ties the two
        together; a lookup by either name answers the same class however they are reached.

        A name the service's model declares NO error shape for has no class of its own in the
        SDK — the AWS-common refusals every service answers are the ones no model carries —
        so the base class is what it answers and what such a refusal is raised as. `operation`
        narrows that to the errors ONE call can be answered with: a code the operation it names
        declares no shape for is the base class too.
        """
        declared = self._modelled(operation)
        named = None
        for candidate in (shape, code):
            if candidate and candidate in declared:
                named = candidate
                break
        if named is None:
            return ClientError
        held = self._classes.get(named)
        if held is None:
            held = type(str(named), (ClientError,), {})
            self._classes[named] = held
        return held


class _ClientMeta:
    """`client.meta`: what the client was built for."""

    def __init__(self, service_name, region_name, endpoint_url, config=None):
        self.service_model = _ServiceModel(service_name)
        self.region_name = region_name
        self.endpoint_url = endpoint_url
        # `client.meta.config`: the configuration this client was built with, which is where
        # a handler reads back what it asked for.
        self.config = config if config is not None else Config(region_name=region_name)


class _ServiceModel:
    def __init__(self, service_name):
        self.service_name = service_name
        self.service_id = service_name


class Client:
    """A client for one service in one Region."""

    def __init__(
        self,
        service_name,
        region_name=None,
        config=None,
        aws_access_key_id=None,
        aws_secret_access_key=None,
        aws_session_token=None,
        endpoint_url=None,
        **_ignored
    ):
        import os

        self._service_name = service_name
        # Which Region the calls of this client are made in: the one the caller named, else
        # the one its `Config` carries ("The region to use in instantiating the client"),
        # else the Region the invocation itself runs in.
        self._config = config
        self._region_name = (
            region_name
            or getattr(config, "region_name", None)
            or os.environ.get("AWS_REGION")
            or os.environ.get("AWS_DEFAULT_REGION")
            or ""
        )
        # WHICH CREDENTIALS this client's calls are signed with: the ones it was built with,
        # else the invocation's own ([`_supplied_credentials`]).
        self._credentials = _supplied_credentials(
            aws_access_key_id, aws_secret_access_key, aws_session_token
        )
        # WHERE this client's calls are addressed: "The complete URL to use for the
        # constructed client", which the SDK sends every call of this client to. A client
        # built with none is addressed at the service's own regional endpoint, and that is
        # what it reports; an API whose resource is named only by its endpoint — the
        # `@connections` surface of a WebSocket api — is reachable no other way.
        self._configured_endpoint = endpoint_url if endpoint_url else ""
        self._endpoint_url = self._configured_endpoint or (
            "https://%s.%s.amazonaws.com" % (service_name, self._region_name)
        )
        self.exceptions = _ClientExceptions(service_name)
        self.meta = _ClientMeta(
            service_name, self._region_name, self._endpoint_url, self._config
        )

    def __getattr__(self, name):
        if name.startswith("_"):
            raise AttributeError(name)
        # `generate_db_auth_token` is not an operation: the SDK adds it to the RDS client
        # alone, and on every other client the attribute does not exist at all.
        if name == "generate_db_auth_token":
            if self._service_name not in _DB_AUTH_TOKEN_SERVICES:
                raise AttributeError(
                    "'%s' object has no attribute '%s'" % (self._service_name, name)
                )
            return self._db_auth_token
        # The managed transfers the SDK adds to the Amazon S3 client, and to no other client
        # ([`_S3_TRANSFER_METHODS`]).
        if name in _S3_TRANSFER_METHODS:
            if self._service_name not in _S3_TRANSFER_SERVICES:
                raise AttributeError(
                    "'%s' object has no attribute '%s'" % (self._service_name, name)
                )
            return getattr(self, "_transfer_" + name)
        operation_name = _operation_name(name)

        def method(**parameters):
            return self._call(operation_name, parameters)

        method.__name__ = name
        return method

    def can_paginate(self, operation_name):
        """Whether this operation is one the SDK pages through.

        An operation the service's own model states no pagination for has no paginator at
        all, which is what a caller asking this is told.
        """
        return bool(self._pagination(operation_name))

    def _db_auth_token(self, DBHostname, Port, DBUsername, Region=None):
        """The authentication token a database login presents in place of a password.

        Not an API call: the SDK builds the token in the client, by presigning a request
        to connect as one database user on one endpoint. It is minted by the runtime,
        which holds the credential it is signed with.
        """
        answered = json.loads(
            _host.db_auth_token(
                str(DBHostname),
                int(Port),
                str(DBUsername),
                Region or self._region_name,
                self._credentials,
            )
        )
        refused = answered.get("refused")
        if refused is not None:
            raise SdkCallNotMade(detail=answered.get("detail") or "")
        return answered.get("token") or ""

    def get_paginator(self, operation_name):
        """The paginator of one operation, which pages it through as the SDK's own does.

        Which members it drives is the called operation's own model's; one page is one call
        of the operation, made through this same client.
        """
        config = self._pagination(operation_name)
        if not config:
            raise OperationNotPageableError(operation_name=operation_name)
        from botocore.paginate import Paginator

        return Paginator(
            getattr(self, operation_name), _operation_name(operation_name), config
        )

    def get_waiter(self, waiter_name):
        """The waiter of that name, which polls this client until the state it waits for.

        WHICH operation it polls, what it accepts and how long it waits are the service's own
        waiter definition's; a state this service publishes no waiter for does not exist,
        which is what the SDK says rather than polling something else.
        """
        # Imported here rather than at module scope, like the paginator above: a handler that
        # waits for nothing pays nothing for the waiter interface's own module.
        from botocore.waiter import waiter_for

        return waiter_for(self, self._service_name, waiter_name)

    @property
    def waiter_names(self):
        """The waiters this client has, under the names the SDK reports them by."""
        from botocore.waiter import waiter_names

        return waiter_names(self._service_name)

    def _pagination(self, operation_name):
        """How the named operation is paged through, as its own model states it."""
        return json.loads(
            _host.pagination(self._service_name, _operation_name(operation_name))
        )

    def close(self):
        return None

    def _transfer_upload_fileobj(
        self, Fileobj, Bucket, Key, ExtraArgs=None, Callback=None, Config=None
    ):
        """`upload_fileobj`: "Upload a file-like object to S3."

        The object is read from where it stands ([`_file_like_bytes`]) and written with one
        `PutObject`, carrying every member `ExtraArgs` names ("Extra arguments that may be
        passed to the client operation") — which is how a transfer states the object's
        `ContentType`, its metadata or its encryption. `Callback` is told how many bytes were
        transferred, as a transfer's progress callback is.

        "The file-like object must be in binary mode": the transfer itself checks that the
        value it was handed is one it can read, before it reads anything, and says which
        method it must carry.
        """
        if not callable(getattr(Fileobj, "read", None)):
            raise ValueError("Fileobj must implement read")
        carried = _file_like_bytes(Fileobj)
        if carried is None:
            raise ParamValidationError(report=_blob_type_report("Fileobj", Fileobj))
        parameters = dict(ExtraArgs or {})
        parameters.update(Bucket=Bucket, Key=Key, Body=carried)
        self._call("PutObject", parameters)
        if Callback is not None:
            Callback(len(carried))
        return None

    def _transfer_download_fileobj(
        self, Bucket, Key, Fileobj, ExtraArgs=None, Callback=None, Config=None
    ):
        """`download_fileobj`: "Download an object from S3 to a file-like object."

        One `GetObject`, whose body is written to the object the caller handed in — which is
        where a handler that reads an object it then parses in memory puts it. The value is
        checked for the method it has to carry first, as the upload above is.
        """
        if not callable(getattr(Fileobj, "write", None)):
            raise ValueError("Fileobj must implement write")
        parameters = dict(ExtraArgs or {})
        parameters.update(Bucket=Bucket, Key=Key)
        answered = self._call("GetObject", parameters)
        # The answer side hands a blob over as a stream or as its own bytes ([`_blob_bytes`]
        # reads either), so a `Body` that is neither is no object this transfer can write:
        # writing nothing and reporting success would be indistinguishable from an empty
        # object, which is a different answer.
        carried = _blob_bytes(answered.get("Body"))
        if carried is None:
            raise SdkCallNotMade(
                detail="GetObject answered a Body this SDK cannot read as bytes"
            )
        Fileobj.write(carried)
        if Callback is not None:
            Callback(len(carried))
        return None

    def _transfer_upload_file(
        self, Filename, Bucket, Key, ExtraArgs=None, Callback=None, Config=None
    ):
        """`upload_file`: "Upload a file to an S3 object", named by its path.

        The path is the function's own — the ephemeral storage a handler writes its working
        files to — and the upload is the one above over the file's own bytes. WHICH paths a
        run can open at all is the runtime's own file layer's to decide, not this transfer's:
        `open` dispatches there, so a path that layer does not serve is refused by it.
        """
        with open(Filename, "rb") as held:
            return self._transfer_upload_fileobj(
                held, Bucket, Key, ExtraArgs=ExtraArgs, Callback=Callback, Config=Config
            )

    def _transfer_download_file(
        self, Bucket, Key, Filename, ExtraArgs=None, Callback=None, Config=None
    ):
        """`download_file`: "Download an S3 object to a file", named by its path.

        The object is fetched BEFORE the path is opened, so a download the service refused
        leaves whatever stood at that path untouched rather than truncated — the ordering a
        transfer that stages its download has.
        """
        import io

        buffered = io.BytesIO()
        self._transfer_download_fileobj(
            Bucket, Key, buffered, ExtraArgs=ExtraArgs, Callback=Callback, Config=Config
        )
        with open(Filename, "wb") as held:
            held.write(buffered.getvalue())
        return None

    def generate_presigned_url(
        self, ClientMethod, Params=None, ExpiresIn=3600, HttpMethod=None
    ):
        """A URL that carries its own signature, as botocore presigns one.

        The URL is built and SIGNED by the runtime, out of the operation's own model and a
        credential the handler never holds, so what comes back addresses the service this
        client was built for and carries a signature over exactly that request.
        """
        operation_name = _operation_name(ClientMethod)
        try:
            expires = int(ExpiresIn)
        except (TypeError, ValueError):
            raise ParamValidationError(
                report="ExpiresIn must be a number of seconds, got %r" % (ExpiresIn,)
            )
        answered = json.loads(
            _host.presign(
                self._service_name,
                operation_name,
                self._region_name,
                json.dumps(Params or {}, default=_encoded),
                expires,
                self._credentials,
            )
        )
        refused = answered.get("refused")
        if refused is not None:
            self._raise_refusal(refused, answered, operation_name)
        return answered.get("url")

    def _input(self, parameters):
        """The input as the operation's own model takes it.

        The SDK renders a few members a caller may give in a friendlier form than the wire
        carries: `CopySource` is the one this SDK renders ([`_copy_source`]), because a copy
        is routinely written with the bucket and the key apart.
        """
        source = parameters.get("CopySource")
        if isinstance(source, dict):
            parameters = dict(parameters, CopySource=_copy_source(source))
        return parameters

    def _call(self, operation_name, parameters):
        parameters = self._input(parameters)
        answered = json.loads(
            _host.aws(
                self._service_name,
                operation_name,
                self._region_name,
                json.dumps(self._sent(parameters), default=_encoded),
                self._credentials,
                self._configured_endpoint,
            )
        )
        refused = answered.get("refused")
        if refused is not None:
            self._raise_refusal(refused, answered, operation_name)
        status = answered.get("status", 0)
        text = answered.get("body") or ""
        try:
            document = json.loads(text) if text else {}
        except ValueError:
            document = {}
        if not isinstance(document, dict):
            document = {}
        metadata = {
            "RequestId": answered.get("requestId"),
            "HTTPStatusCode": status,
            "RetryAttempts": 0,
        }
        if 200 <= status < 300:
            # The members the ANSWER says carry bytes are handed over as the stream a
            # client answers one with, decoded from the base64 they travelled as.
            for member in answered.get("streams") or ():
                carried = document.get(member)
                if isinstance(carried, str):
                    document[member] = StreamingBody(base64.b64decode(carried))
            # The member bound to the WHOLE body whose blob is not a streaming one is handed
            # over the same way: this SDK reads the payload of every output whose payload
            # shape is a blob as a stream, whatever that shape's own traits say.
            for member in answered.get("payloadBlobs") or ():
                carried = document.get(member)
                if isinstance(carried, str):
                    document[member] = StreamingBody(base64.b64decode(carried))
            # Every other value the answer says carries bytes is handed over as those
            # bytes, which is what the SDK gives a caller for a blob member: a JSON
            # protocol travels one as base64 text, so the text is decoded wherever the
            # answer places it — at whatever depth, inside a list or a map as well.
            for path in answered.get("blobs") or ():
                _materialise(document, path, _bytes_of)
            # Every value the answer says carries an INSTANT is handed over as the native
            # date-time the SDK gives a caller for a member the service's model types as a
            # timestamp: the protocol carries one as the seconds since the epoch, so a handler
            # reads an instant it can format and compare rather than that number — at whatever
            # depth the answer places it, inside a list or a map as well.
            for path in answered.get("instants") or ():
                _materialise(document, path, _instant_of)
            document["ResponseMetadata"] = metadata
            return document
        shape = _error_shape(answered.get("errorType"), document)
        code = _error_code(answered.get("queryErrorCode"), shape)
        response = {
            "Error": {"Code": code, "Message": _error_message(document)},
            "ResponseMetadata": metadata,
        }
        # The members the refusal's own error SHAPE declares stand at the TOP of the answer it
        # is raised with, beside `Error` and `ResponseMetadata`: an error shape is a structure
        # like any other, so a handler reads `error.response["CancellationReasons"]` — or any
        # other member its model gives that class — the way it reads an answered call's own
        # members. WHICH members one carries, and the key the wire carries each of them in, is
        # the called service's model's to say ([`super::aws_api::Answer::error_members`]); the
        # value is the service's own, exactly as it answered it.
        for member, carried in (answered.get("errorMembers") or {}).items():
            held, present = _member_of(document, carried)
            if present:
                response[member] = held
        raise self.exceptions.from_code(code, shape, operation_name)(response, operation_name)

    def _sent(self, parameters):
        """The call's parameters, with every byte-carrying member encoded as base64."""
        members = _BLOB_PARAMETERS.get(self._service_name.replace("-", ""), ())
        if not members:
            return parameters
        sent = dict(parameters)
        for member in members:
            if member not in sent:
                continue
            carried = _blob_bytes(sent[member])
            if carried is None:
                raise ParamValidationError(
                    report=_blob_type_report(member, sent[member])
                )
            sent[member] = base64.b64encode(carried).decode("ascii")
        return sent

    def _raise_refusal(self, refused, answered, operation_name):
        detail = answered.get("detail") or ""
        if refused == "unknown_service":
            raise UnknownServiceError(
                service_name=self._service_name, known_service_names=detail
            )
        if refused == "unknown_operation":
            # An SDK carries a method per operation its service's model declares and no other,
            # so a method naming no operation of that service does not exist on the client at
            # all: the attribute is missing, which is what the SDK says, and the name it names
            # is the one the caller asked for.
            raise AttributeError(
                "'%s' object has no attribute '%s'"
                % (self._service_name, xform_name(operation_name))
            )
        if refused == "timed_out":
            raise ReadTimeoutError(endpoint_url=self._endpoint_url)
        raise SdkCallNotMade(detail=detail)
"##;

/// `boto3`: the names a handler imports.
const BOTO3_SOURCE: &str = r#"
"""The AWS SDK for Python, as this runtime supplies it."""

from boto3.session import Session

# The GENERATION of the SDK these modules implement, reported the way the SDK reports its own
# (`boto3.__version__`), and the same generation its lower half reports
# (`botocore.__version__`).
__version__ = "1.0.0"


def client(*args, **kwargs):
    """A client for one service, as `boto3.client(...)` builds one."""
    return Session().client(*args, **kwargs)


def resource(*args, **kwargs):
    """A service resource, as `boto3.resource(...)` builds one."""
    return Session().resource(*args, **kwargs)


def setup_default_session(**kwargs):
    return None
"#;

/// `boto3.session`: the session a client is built from.
const BOTO3_SESSION_SOURCE: &str = r#"
"""The session a client is built from."""

from botocore.client import Client
from botocore.credentials import Credentials
from botocore.exceptions import SdkCallNotMade

# The services this runtime's SDK supplies resource classes for.
_RESOURCE_SERVICES = ("dynamodb",)


class Session:
    def __init__(
        self,
        aws_access_key_id=None,
        aws_secret_access_key=None,
        aws_session_token=None,
        region_name=None,
        **_ignored
    ):
        import os

        self.region_name = (
            region_name
            or os.environ.get("AWS_REGION")
            or os.environ.get("AWS_DEFAULT_REGION")
            or None
        )
        # The credentials every client this session builds is given, unless the call that
        # builds one names credentials of its own: a session built with a vended session is
        # how a handler reaches a service as the identity it assumed.
        self._credentials = {
            "aws_access_key_id": aws_access_key_id,
            "aws_secret_access_key": aws_secret_access_key,
            "aws_session_token": aws_session_token,
        }

    def client(self, service_name, region_name=None, **kwargs):
        # The Region a client calls in, in the order the SDK resolves it: the one this call
        # named, else the one the client's own `Config` carries, else the session's.
        configured = getattr(kwargs.get("config"), "region_name", None)
        return Client(
            service_name,
            region_name=region_name or configured or self.region_name,
            **self._client_credentials(kwargs),
        )

    def _client_credentials(self, kwargs):
        """The client's keyword arguments with this session's credentials filled in.

        A call that names credentials of its own is left exactly as it asked; otherwise the
        client is built with the session's, which is the order the SDK resolves them in.
        """
        if any(name in kwargs for name in self._credentials):
            return kwargs
        return dict(kwargs, **self._credentials)

    def get_credentials(self):
        """The credentials this session's clients sign with, as the SDK resolves them.

        A session built with credentials of its own answers those; a session built with none
        resolves the DEFAULT credential chain, which in a Lambda execution environment is the
        three reserved variables the runtime exported — the temporary credentials of the
        session of the function's execution role, session token included. `None` where
        nothing resolves at all, which is what the SDK's own chain answers then.
        """
        from botocore.credentials import resolve_environment_credentials

        if (
            self._credentials["aws_access_key_id"] is None
            and self._credentials["aws_secret_access_key"] is None
        ):
            return resolve_environment_credentials()
        return Credentials(
            access_key=self._credentials["aws_access_key_id"],
            secret_key=self._credentials["aws_secret_access_key"],
            token=self._credentials["aws_session_token"],
        )

    def resource(self, service_name, region_name=None, **kwargs):
        if service_name in _RESOURCE_SERVICES:
            # Imported here rather than at module scope: a handler that only builds
            # clients pays nothing for the resource interface's own modules.
            from boto3.dynamodb.table import service_resource

            return service_resource(
                self.client(service_name, region_name=region_name, **kwargs)
            )
        raise SdkCallNotMade(
            detail="the AWS SDK this runtime supplies has resource classes for %s only; "
            "build a client with boto3.client(%r) instead"
            % (", ".join(sorted(_RESOURCE_SERVICES)), service_name)
        )
"#;
