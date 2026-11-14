// SPDX-License-Identifier: CC0-1.0

//! The HTTP a running handler makes ITSELF: one request, carried by the host into this same
//! emulator.
//!
//! A function does not only call control planes through an SDK. The data plane of a search
//! domain, a document store or a queue's own endpoint is reached by SIGNING an HTTP request
//! and sending it — which is what the AWS SDK's own signer
//! ([`super::aws_sdk_guest`]'s `botocore.auth`) and the standard library's HTTP client exist
//! for, and what a handler that indexes or searches documents does. This module is the host
//! half of that: the guest hands over a method, a URL, headers and a body, and the host
//! builds that request, signs it, and dispatches it through the in-process callback
//! ([`::smithy_server_runtime::EmbeddedEmulator`]) into this process's router. The answer is
//! the emulated service's own answer.
//!
//! What the guest can and cannot do here:
//!
//! * The destination is ALWAYS this emulator, and always an endpoint it PUBLISHES: the URL
//!   has to name one — a host of AWS's own naming, whose endpoint prefix names the service
//!   ([`endpoint_prefix_of`]), this emulator's own endpoint with a data plane named in the
//!   path under `service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`, or the
//!   `host:port` the core published a RESOURCE's own listener at
//!   ([`addressed_resource_plane`]) — the address a search collection, a database or a
//!   gateway reports in its control plane for a client to dial, which is an ADDRESS and so
//!   names no service in its host: WHICH service answers there is what the service that
//!   opened the listener recorded (`service_common::published_data_planes`). A URL naming
//!   none of them is REFUSED and nothing is sent. No host name is resolved and no socket is
//!   opened on any path: the request is handed to the callback transport, so it cannot leave
//!   this process.
//! * The guest never sees a credential. The signature is put on by host code with the
//!   credential the framework hands this emulator's handlers, exactly as the SDK seam signs
//!   its calls ([`super::aws_api`]), and the authorization headers the guest supplied are
//!   DROPPED rather than forwarded, so a handler cannot have a request signed as anything
//!   else.
//! * Every bound is the host's: which methods may be sent, how large the request and the
//!   answer may be, how many requests one run may make and how long one may take — the last
//!   two shared with the SDK seam, because they bound the RUN rather than one call
//!   ([`super::python`]).
//!
//! Nothing about an answer is synthesised: the status, the headers and the body the guest
//! reads back are the ones the emulated service answered, and a request this module cannot
//! make is reported as one that was not made.

use ::std::time::Duration;

use ::service_common::data_plane_addressing::{
    first_segment, host_only, EMULATOR_PATH_NAMESPACE,
};
use ::smithy_server_runtime::EmbeddedEmulator;

use super::aws_api::{Refusal, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES};
use super::package::PackageModule;

/// The methods a handler may send.
///
/// The ones HTTP defines for a request with this shape; a value that is not one of them is
/// not a method, so no request is built for it and nothing of the guest's string reaches a
/// request line.
const METHODS: &[&str] = &[
    "GET", "PUT", "POST", "DELETE", "HEAD", "PATCH", "OPTIONS",
];

/// The headers the HOST decides, which a guest-supplied set never carries into the request:
/// the ones that say who the request is from (replaced by the host's own signature), the
/// ones that describe the connection, and the ones the framing is computed from.
const HOST_OWNED_HEADERS: &[&str] = &[
    "authorization",
    "x-amz-date",
    "x-amz-content-sha256",
    "x-amz-security-token",
    "host",
    "content-length",
    "connection",
    "transfer-encoding",
    "expect",
    "upgrade",
    "proxy-authorization",
];

/// What one request came back with: the emulated service's own answer.
#[derive(Debug, Clone)]
pub struct Answered {
    /// The HTTP status it answered.
    pub status: u16,
    /// The headers it carried, in the order it carried them.
    pub headers: Vec<(String, String)>,
    /// Its body, as the bytes it answered.
    pub body: Vec<u8>,
}

/// ONE request a handler made, as the host is handed it.
#[derive(Debug)]
pub struct Sending<'a> {
    /// The in-process callback it is dispatched through.
    pub callback: &'a EmbeddedEmulator,
    /// The Region a URL naming none of its own is signed for: the invocation's.
    pub default_region: &'a str,
    /// The method the handler named.
    pub method: &'a str,
    /// The URL it addressed.
    pub url: &'a str,
    /// The headers it built, as name/value pairs.
    pub headers: &'a [(String, String)],
    /// The body it sent.
    pub body: &'a [u8],
    /// What the run has left of its deadline, which bounds this request: it cannot outlast
    /// the invocation that made it.
    pub within: Duration,
    /// WHO it is made as: the credential the invocation was given
    /// ([`super::aws_api::CallerIdentity`]).
    pub identity: Option<&'a super::aws_api::CallerIdentity>,
    /// Which service answers at an address this emulator published for a resource's own
    /// data plane, for a URL that addresses one ([`addressed_resource_plane`]). `None`
    /// where this emulator vends no such records, and then only the two addressings that
    /// name a service in the URL itself resolve.
    pub published: Option<&'a ::service_common::published_data_planes::PublishedPlanes>,
}

/// Make ONE request, as `sending` states it.
pub async fn sent(sending: &Sending<'_>) -> Result<Answered, Refusal> {
    let method = sending.method.to_ascii_uppercase();
    if !METHODS.contains(&method.as_str()) {
        return Err(Refusal::NotMade(format!(
            "{method} is not an HTTP method a handler may send"
        )));
    }
    if sending.body.len() > MAX_REQUEST_BYTES {
        return Err(Refusal::RequestTooLarge);
    }
    let addressed = addressed(sending.url, sending.default_region)
        .or_else(|| {
            addressed_resource_plane(sending.url, sending.published, sending.default_region)
        })
        .ok_or(Refusal::UnknownService)?;
    match ::tokio::time::timeout(
        sending.within,
        dispatch(
            sending.callback,
            &addressed,
            &method,
            sending.headers,
            sending.body,
            sending.identity,
        ),
    )
    .await
    {
        Ok(answer) => answer,
        Err(_elapsed) => Err(Refusal::TimedOut),
    }
}

/// WHERE one URL addresses: the service of this emulator it names, the host and the path it
/// is sent to, and the Region its signature is scoped to.
#[derive(Debug, PartialEq, Eq)]
struct Addressed {
    /// The signing name of the service the address names, which is also the credential
    /// scope that routes the request to it inside this emulator.
    signing_name: String,
    /// The host the request is addressed to, as the URL names it.
    host: String,
    /// The path and query it is addressed to.
    target: String,
    /// The Region the signature is scoped to.
    region: String,
}

/// Read `url` as the address of a service of THIS emulator, or `None` for a URL that names
/// none — which is a request this seam does not make.
fn addressed(url: &str, default_region: &str) -> Option<Addressed> {
    let (authority, target) = authority_and_target(url)?;
    // A host of AWS's OWN naming names the service in it: the endpoint prefix, which is
    // also what a request to that service is signed for.
    let host = host_only(authority);
    if let Some(prefix) = endpoint_prefix_of(host) {
        return Some(Addressed {
            signing_name: prefix,
            host: authority.to_string(),
            target,
            region: region_of(host).unwrap_or_else(|| default_region.to_string()),
        });
    }
    // A host the front door itself resolves to a service is NOT read here: its reader
    // answers the first label of ANY host, so reading it would make every address resolve
    // to something. The two addressings below are the ones this emulator really serves.
    // Otherwise this is the emulator's own endpoint, where a data plane is named in the
    // path: `/_aws/<plane>/…`, which is the address such a plane publishes for itself.
    let (namespace, under) = first_segment(&target)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, _) = first_segment(under)?;
    if plane.is_empty() || !plane.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    let signing_name = plane.to_string();
    Some(Addressed {
        signing_name,
        host: authority.to_string(),
        target,
        region: default_region.to_string(),
    })
}

/// The authority and the request target `url` names, or `None` for a value that is no
/// HTTP(S) URL at all — which addresses nothing and is a request this seam does not make.
///
/// Read by hand rather than with a URL parser because what the guest handed over is a
/// STRING: a value that is not a URL has to come back as "no address" rather than as a
/// parser's own error.
fn authority_and_target(url: &str) -> Option<(&str, String)> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let (authority, path) = match rest.find(['/', '?', '#']) {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || !::service_common::data_plane_addressing::is_host_shaped(authority) {
        return None;
    }
    let target = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    Some((authority, target))
}

/// WHERE a URL addressing a RESOURCE's own published listener goes, or `None` for one that
/// addresses no such listener.
///
/// A resource whose data plane speaks its own protocol is published on a listener the CORE
/// opened for it (`engine.listeners()`), and what the owning service reports for a client to
/// dial is that listener's `host:port` — an ADDRESS on the machine this emulator runs on,
/// which is the only address a client here resolves. A handler reading an endpoint out of a
/// control-plane response (a collection's `collectionEndpoint`, a cluster's `Endpoint`) was
/// told to dial exactly that.
///
/// Nothing about such an address names a service: its host is an address, so the two readings
/// above answer nothing for it. WHICH service answers there is what the service that OPENED
/// the listener recorded ([`::service_common::published_data_planes`]), so the request is
/// signed for — and routed inside this emulator to — the service that published the very
/// address the handler dialled, and an address this emulator published nothing at stays a
/// request this seam does not make. The Region is the invocation's own: a listener of this
/// emulator is not an endpoint of another Region.
fn addressed_resource_plane(
    url: &str,
    published: Option<&::service_common::published_data_planes::PublishedPlanes>,
    default_region: &str,
) -> Option<Addressed> {
    let (authority, target) = authority_and_target(url)?;
    let signing_name = published?.answering_service(authority)?;
    Some(Addressed {
        signing_name,
        host: authority.to_string(),
        target,
        region: default_region.to_string(),
    })
}

/// The ENDPOINT PREFIX an AWS-shaped host names, which is what a request to that service is
/// signed for.
///
/// AWS names a service's own endpoint `<prefix>.<region>.amazonaws.com` and a RESOURCE's own
/// host either `<resource>.<region>.<prefix>.amazonaws.com` (a search domain) or
/// `<resource>.<prefix>.<region>.amazonaws.com` (a bucket), so the prefix is the label
/// NEXT TO the Region one ([`region_of`]) — after it when the Region is the last label
/// before `amazonaws.com`, and before it otherwise. A host naming no Region at all is a
/// global endpoint, whose prefix is the label before `amazonaws.com`.
///
/// `None` for a host that is not AWS's own naming — this emulator's own endpoint, an
/// address of anything else — which names no service this way.
fn endpoint_prefix_of(host: &str) -> Option<String> {
    let labels: Vec<&str> = host.split('.').collect();
    let [service @ .., "amazonaws", "com"] = labels.as_slice() else {
        return None;
    };
    let at = service.iter().position(|label| region_shaped(label));
    let prefix = match at {
        Some(at) if at + 1 < service.len() => service[at + 1],
        Some(at) if at > 0 => service[at - 1],
        Some(_) => return None,
        None => service.last().copied()?,
    };
    (!prefix.is_empty()
        && prefix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
    .then(|| prefix.to_string())
}

/// The Region an AWS-shaped host names: the label of it that is a REGION NAME.
///
/// A host of AWS's own naming carries the Region as one label among the service's
/// (`s3.<region>.amazonaws.com`, `search-things-abc.<region>.es.amazonaws.com`,
/// `<bucket>.s3.<region>.amazonaws.com`), so WHICH label it is cannot be read off a fixed
/// position; what every one of them has is exactly one label shaped like a Region name, and
/// that is the one taken. `None` for a host carrying none, whose request is signed for the
/// Region the invocation itself was addressed to.
fn region_of(host: &str) -> Option<String> {
    host.split('.')
        .rev()
        .find(|label| region_shaped(label))
        .map(str::to_string)
}

/// Whether `label` is shaped like a Region name: lower-case letters, then one or more
/// hyphen-separated words, then a number — `us-west-2`, `ap-southeast-4`, `eu-central-1`.
fn region_shaped(label: &str) -> bool {
    let mut parts = label.split('-');
    let Some(partition) = parts.next() else {
        return false;
    };
    let rest: Vec<&str> = parts.collect();
    let Some((number, words)) = rest.split_last() else {
        return false;
    };
    !partition.is_empty()
        && partition.bytes().all(|byte| byte.is_ascii_lowercase())
        && !words.is_empty()
        && words
            .iter()
            .all(|word| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase()))
        && !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
}

/// Build, sign and dispatch the request, and read the answer back.
async fn dispatch(
    callback: &EmbeddedEmulator,
    addressed: &Addressed,
    method: &str,
    headers: &[(String, String)],
    body: &[u8],
    identity: Option<&super::aws_api::CallerIdentity>,
) -> Result<Answered, Refusal> {
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{}{}", addressed.host, addressed.target))
        .header("host", addressed.host.clone());
    for (name, value) in headers {
        let lower = name.to_ascii_lowercase();
        if HOST_OWNED_HEADERS.contains(&lower.as_str()) {
            continue;
        }
        // A name or a value that is not one a header can carry is not sent: nothing of the
        // guest's text reaches the request's header structure except as a header.
        let (Ok(name), Ok(value)) = (
            ::http::HeaderName::try_from(lower.as_str()),
            ::http::HeaderValue::try_from(value.as_str()),
        ) else {
            continue;
        };
        builder = builder.header(name, value);
    }
    let mut request = builder
        .body(body.to_vec())
        .map_err(|error| Refusal::NotMade(error.to_string()))?;
    super::aws_api::signed(
        &mut request,
        &addressed.signing_name,
        &addressed.region,
        callback,
        identity,
    )
    .await
    .map_err(Refusal::NotMade)?;
    let (parts, body) = request.into_parts();
    let request = ::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    );
    let request = ::aws_smithy_runtime_api::http::Request::try_from(request)
        .map_err(|error| Refusal::NotMade(error.to_string()))?;
    let response = ::aws_smithy_runtime_api::client::http::HttpConnector::call(
        &callback.http_connector(),
        request,
    )
    .await
    .map_err(|error| Refusal::NotMade(error.to_string()))?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let payload = response.body().bytes().ok_or_else(|| {
        Refusal::NotMade("the in-process answer carried a streaming body".to_string())
    })?;
    if payload.len() > MAX_RESPONSE_BYTES {
        return Err(Refusal::ResponseTooLarge);
    }
    Ok(Answered {
        status,
        headers,
        body: payload.to_vec(),
    })
}

/// The modules of the standard library this RUNTIME supplies in place of the frozen ones:
/// the socket module a Python program expects its operating system to have, the HTTP
/// client that is built on one, and the TLS layer a program imports beside them
/// ([`super::guest_tls`]).
///
/// Resolved before the frozen standard library and after the deployment package's own
/// entries, so these are the `socket`, `http.client` and `ssl` a handler imports, and a
/// package shipping its own still wins.
///
/// The socket module is [`SOCKET_SOURCE`] and the DATAGRAM half of it
/// ([`super::xray_daemon::DATAGRAM_SOURCE`]) together: the names, the errors and the
/// conversions a program reads off `socket` are the former's, and the sockets themselves are
/// the latter's — so there is ONE `socket` a handler can import, whose only reachable
/// destination is the in-process X-Ray daemon.
#[must_use]
pub fn runtime_stdlib_modules() -> Vec<PackageModule> {
    [
        (
            "socket.py",
            format!(
                "{SOCKET_SOURCE}{}",
                super::xray_daemon::DATAGRAM_SOURCE
            ),
        ),
        ("http/client.py", HTTP_CLIENT_SOURCE.to_string()),
    ]
    .into_iter()
    .map(|(path, source)| PackageModule {
        path: path.to_string(),
        source,
    })
    .chain(super::guest_tls::runtime_stdlib_modules())
    .collect()
}

/// `socket`: the NAMES, errors and conversions a Python program reads off the module it
/// reaches a socket through. The sockets themselves are the datagram half appended to this
/// source ([`super::xray_daemon::DATAGRAM_SOURCE`]).
///
/// A Lambda function in this emulator has no host network: there is no name to resolve and no
/// address to connect to, so every call that would do one of those raises the error a Python
/// program handles for it, and the one address a datagram can reach is the in-process X-Ray
/// daemon. The NAMES are all here — the address families, the socket types, the option levels,
/// the errors and the default timeout — so a module that imports `socket` to name one of them
/// imports cleanly, which is what the standard library's own HTTP, email and URL modules do.
const SOCKET_SOURCE: &str = r#"
"""The socket interface of this runtime: no host socket is reachable from a function."""

AF_UNSPEC = 0
AF_INET = 2
AF_INET6 = 10
SOCK_STREAM = 1
SOCK_DGRAM = 2
SOL_SOCKET = 1
SO_REUSEADDR = 2
SO_KEEPALIVE = 9
SO_ERROR = 4
IPPROTO_IP = 0
IPPROTO_TCP = 6
IPPROTO_UDP = 17
TCP_NODELAY = 1
SHUT_RD = 0
SHUT_WR = 1
SHUT_RDWR = 2
AI_PASSIVE = 1
AI_CANONNAME = 2
NI_NUMERICHOST = 1
has_ipv6 = False

# What `socket.timeout` and `socket.error` are in a current Python: the builtin errors,
# which is what a caller catches.
error = OSError
timeout = TimeoutError


class herror(OSError):
    """A name service failure."""


class gaierror(OSError):
    """An address resolution failure."""


# The sentinel the standard library passes to mean "the default timeout", which modules
# such as `http.client` read off this module.
_GLOBAL_DEFAULT_TIMEOUT = object()

_UNREACHABLE = "a Lambda function in this emulator has no host network to reach"


def getdefaulttimeout():
    return None


def setdefaulttimeout(timeout):
    return None


def socketpair(*_arguments, **_keywords):
    raise error(_UNREACHABLE)


def create_connection(*_arguments, **_keywords):
    raise error(_UNREACHABLE)


def create_server(*_arguments, **_keywords):
    raise error(_UNREACHABLE)


def getaddrinfo(*_arguments, **_keywords):
    raise gaierror(_UNREACHABLE)


def getnameinfo(*_arguments, **_keywords):
    raise gaierror(_UNREACHABLE)


def gethostbyname(*_arguments, **_keywords):
    raise gaierror(_UNREACHABLE)


def gethostbyname_ex(*_arguments, **_keywords):
    raise gaierror(_UNREACHABLE)


def gethostbyaddr(*_arguments, **_keywords):
    raise gaierror(_UNREACHABLE)


def gethostname():
    raise error(_UNREACHABLE)


def getfqdn(name=""):
    raise error(_UNREACHABLE)


def inet_aton(address):
    """The four bytes of a dotted-quad address. A pure conversion: it resolves nothing."""
    parts = address.split(".")
    if len(parts) != 4:
        raise error("illegal IP address string passed to inet_aton")
    numbers = []
    for part in parts:
        if not part.isdigit() or not 0 <= int(part) <= 255:
            raise error("illegal IP address string passed to inet_aton")
        numbers.append(int(part))
    return bytes(numbers)


def inet_ntoa(packed):
    """The dotted quad four bytes spell, the other way round."""
    if len(packed) != 4:
        raise error("packed IP wrong length for inet_ntoa")
    return ".".join(str(byte) for byte in packed)


def htons(value):
    return ((value & 0xFF) << 8) | ((value >> 8) & 0xFF)


ntohs = htons


def htonl(value):
    return (
        ((value & 0xFF) << 24)
        | ((value & 0xFF00) << 8)
        | ((value >> 8) & 0xFF00)
        | ((value >> 24) & 0xFF)
    )


ntohl = htonl
"#;

/// `http.client`: the HTTP client of this runtime.
///
/// The surface is the standard library's — a connection you make a request on and read a
/// response from, with the same class and attribute names, so `urllib.request` and a handler
/// using the client directly both work — and the TRANSPORT is the host seam above: one
/// signed request into this emulator per `getresponse`. Nothing here opens a socket, because
/// there is none to open; a URL this emulator serves no service for is reported as the
/// connection failure it is.
const HTTP_CLIENT_SOURCE: &str = r##"
"""The HTTP client of this runtime: a request is carried by the runtime, not by a socket."""

import base64
import email.message
import http
import io
import json
import socket

_host = __import__("_lambda_host")

HTTP_PORT = 80
HTTPS_PORT = 443

# The methods a request carries a body on, which is what decides the `Content-Length` of a
# request with no body at all.
_METHODS_EXPECTING_BODY = frozenset(("PATCH", "POST", "PUT"))

# The reason phrase of each status, as the standard library's own mapping holds it.
responses = dict((status.value, status.phrase) for status in http.HTTPStatus)

globals().update((status.name, status.value) for status in http.HTTPStatus)


class HTTPException(Exception):
    pass


class NotConnected(HTTPException):
    pass


class InvalidURL(HTTPException):
    pass


class UnknownProtocol(HTTPException):
    pass


class UnknownTransferEncoding(HTTPException):
    pass


class BadStatusLine(HTTPException):
    pass


class LineTooLong(HTTPException):
    pass


class RemoteDisconnected(ConnectionResetError, BadStatusLine):
    pass


class ResponseNotReady(HTTPException):
    pass


class CannotSendRequest(HTTPException):
    pass


class CannotSendHeader(HTTPException):
    pass


class ImproperConnectionState(HTTPException):
    pass


class HTTPMessage(email.message.Message):
    """The headers of one answer, as the standard library's own class."""

    def getallmatchingheaders(self, name):
        name = name.lower() + ":"
        return [line for line in self.keys() if line.lower().startswith(name)]


class HTTPResponse(io.BytesIO):
    """One answer, read the way the standard library's response is read.

    The bytes are the ones the answer carried, so this is a reading position over them: a
    `read()` returns what the service answered and nothing is fetched a second time.
    """

    def __init__(self, status, reason, headers, body, url=None, method=None):
        io.BytesIO.__init__(self, body)
        self.status = status
        self.code = status
        self.reason = reason
        self.version = 11
        self.url = url
        self._method = method
        self.headers = HTTPMessage()
        for name, value in headers:
            self.headers[name] = value
        self.msg = self.headers
        self.chunked = False
        self.will_close = False
        self.length = len(body)

    def info(self):
        return self.headers

    def geturl(self):
        return self.url

    def getcode(self):
        return self.status

    def getheader(self, name, default=None):
        value = self.headers.get(name)
        return default if value is None else value

    def getheaders(self):
        return list(self.headers.items())

    def isclosed(self):
        return self.closed

    def begin(self):
        return None

    def fileno(self):
        raise OSError("this response has no file descriptor")

    def flush(self):
        return None


class HTTPConnection:
    """A connection to one host, as the standard library's client builds one.

    No socket is opened: the request built here is handed to the runtime, which signs it
    and carries it into this emulator ([`super::sent`] in the host).
    """

    default_port = HTTP_PORT
    _scheme = "http"
    debuglevel = 0
    # The protocol version a request is made under, which `urllib.request` reads off the
    # class to decide whether it may keep the connection alive.
    _http_vsn = 11
    _http_vsn_str = "HTTP/1.1"
    auto_open = 1

    def __init__(
        self,
        host,
        port=None,
        timeout=socket._GLOBAL_DEFAULT_TIMEOUT,
        source_address=None,
        blocksize=8192,
    ):
        self.host, self.port = self._split(host, port)
        self.timeout = timeout
        self.source_address = source_address
        self.blocksize = blocksize
        self.sock = None
        self._reset()

    def _reset(self):
        self._method = None
        self._path = None
        self._headers = []
        self._body = b""
        self._response = None

    @staticmethod
    def _split(host, port):
        if port is None and host and host.rfind(":") > host.rfind("]"):
            name, _, carried = host.rpartition(":")
            if carried.isdigit():
                return name.strip("[]"), int(carried)
            raise InvalidURL("nonnumeric port: '%s'" % (carried,))
        return host, port

    @property
    def _authority(self):
        if self.port is None or self.port == self.default_port:
            return self.host
        if ":" in self.host and not self.host.startswith("["):
            return "[%s]:%d" % (self.host, self.port)
        return "%s:%d" % (self.host, self.port)

    @staticmethod
    def _get_content_length(body, method):
        """The `Content-Length` a body of this shape gives a request of this method.

        `urllib.request` asks the class for it to decide whether to set the header itself.
        A method that expects a body carries `0` for no body at all; a stream's length is
        not known until it is read.
        """
        if body is None:
            return 0 if method.upper() in _METHODS_EXPECTING_BODY else None
        if hasattr(body, "read"):
            return None
        if isinstance(body, str):
            return len(body.encode("utf-8"))
        try:
            return len(body)
        except TypeError:
            return None

    def set_debuglevel(self, level):
        self.debuglevel = level

    def set_tunnel(self, host, port=None, headers=None):
        raise OSError("a Lambda function in this emulator cannot tunnel a connection")

    def connect(self):
        return None

    def close(self):
        self._reset()

    def putrequest(self, method, url, skip_host=False, skip_accept_encoding=False):
        self._reset()
        self._method = method
        self._path = url

    def putheader(self, header, *values):
        for value in values:
            if isinstance(value, bytes):
                value = value.decode("latin-1")
            self._headers.append((str(header), str(value)))

    def endheaders(self, message_body=None, *, encode_chunked=False):
        if message_body is not None:
            self.send(message_body)

    def send(self, data):
        self._body += _as_bytes(data)

    def request(self, method, url, body=None, headers=None, *, encode_chunked=False):
        self.putrequest(method, url)
        for header, value in (headers or {}).items():
            self.putheader(header, value)
        self.endheaders(body)

    def getresponse(self):
        if self._method is None or self._path is None:
            raise ResponseNotReady()
        url = "%s://%s%s" % (self._scheme, self._authority, self._path)
        answered = json.loads(
            _host.request(
                self._method,
                url,
                json.dumps(self._headers),
                base64.b64encode(self._body).decode("ascii"),
            )
        )
        refused = answered.get("refused")
        if refused is not None:
            detail = answered.get("detail") or ""
            if refused == "timed_out":
                raise socket.timeout(detail)
            if refused == "unknown_service":
                raise socket.gaierror(
                    "%s addresses no service this emulator serves%s"
                    % (url, ": " + detail if detail else "")
                )
            raise OSError(detail or "the request was not made")
        self._response = HTTPResponse(
            answered.get("status", 0),
            answered.get("reason") or responses.get(answered.get("status", 0), ""),
            [tuple(pair) for pair in answered.get("headers") or []],
            base64.b64decode(answered.get("body") or ""),
            url=url,
            method=self._method,
        )
        return self._response


class HTTPSConnection(HTTPConnection):
    """The same connection for an `https` URL.

    There is no TLS to negotiate — the request never reaches a socket — so a context or a
    certificate a caller passes is accepted and unused; what the request carries instead is
    the runtime's own signature.
    """

    default_port = HTTPS_PORT
    _scheme = "https"

    def __init__(
        self,
        host,
        port=None,
        key_file=None,
        cert_file=None,
        timeout=socket._GLOBAL_DEFAULT_TIMEOUT,
        source_address=None,
        *,
        context=None,
        check_hostname=None,
        blocksize=8192,
    ):
        HTTPConnection.__init__(
            self,
            host,
            port=port,
            timeout=timeout,
            source_address=source_address,
            blocksize=blocksize,
        )
        self.key_file = key_file
        self.cert_file = cert_file


def _create_https_context(http_version=11):
    """The TLS context an `https` client would negotiate with.

    `urllib.request` asks this module for one when a caller passed none. There is no socket
    to negotiate on here and nothing is negotiated, so there is no context either: the
    request's own signature is what it carries instead.
    """
    return None


def _as_bytes(data):
    """The bytes one `send` carries, out of what a caller passed."""
    if data is None:
        return b""
    if isinstance(data, (bytes, bytearray)):
        return bytes(data)
    if isinstance(data, str):
        return data.encode("utf-8")
    read = getattr(data, "read", None)
    if read is not None:
        return _as_bytes(read())
    if isinstance(data, (list, tuple)):
        return b"".join(_as_bytes(one) for one in data)
    raise TypeError("a request body cannot be %r" % (type(data).__name__,))
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the addressing table: a URL, and the service of this emulator it names.
    struct Addressing {
        case: &'static str,
        /// The URL the handler asked for.
        url: &'static str,
        /// The signing name, path and Region it addresses, or `None` for a URL that names
        /// no service of this emulator.
        expected: Option<(&'static str, &'static str, &'static str)>,
    }

    /// WHICH addresses a handler's own request may name, over a table whose rows span what
    /// the rule generalises over: a service's own endpoint host, a resource's own host,
    /// this emulator's own endpoint with a data plane named in the path, and every shape of
    /// address that names no service of this emulator at all.
    #[test]
    fn only_an_address_this_emulator_serves_is_addressed() {
        let cases: &[Addressing] = &[
            Addressing {
                case: "a service's own regional endpoint",
                url: "https://s3.us-west-2.amazonaws.com/bucket/key",
                expected: Some(("s3", "/bucket/key", "us-west-2")),
            },
            Addressing {
                case: "a search domain's own endpoint",
                url: "https://search-things-abc.eu-west-1.es.amazonaws.com/index/_search?q=a",
                expected: Some(("es", "/index/_search?q=a", "eu-west-1")),
            },
            Addressing {
                case: "this emulator's endpoint, with the plane named in the path",
                url: "https://127.0.0.1:9297/_aws/es/things/index/_doc/1",
                expected: Some(("es", "/_aws/es/things/index/_doc/1", "us-west-2")),
            },
            Addressing {
                case: "the same endpoint with no path at all",
                url: "https://127.0.0.1:9297",
                expected: None,
            },
            Addressing {
                case: "this emulator's endpoint naming no plane",
                url: "https://127.0.0.1:9297/index/_search",
                expected: None,
            },
            Addressing {
                case: "a host that is not this emulator's and names no service",
                url: "https://example.test/anything",
                expected: None,
            },
            Addressing {
                case: "a scheme that is not HTTP",
                url: "ftp://127.0.0.1/x",
                expected: None,
            },
            Addressing {
                case: "a value that is no URL at all",
                url: "not a url",
                expected: None,
            },
        ];
        for row in cases {
            let case = row.case;
            let addressed = addressed(row.url, "us-west-2");
            let actual = addressed.as_ref().map(|addressed| {
                (
                    addressed.signing_name.as_str(),
                    addressed.target.as_str(),
                    addressed.region.as_str(),
                )
            });
            assert_eq!(
                actual, row.expected,
                "case {case:?}: expected {:?}, got {actual:?}",
                row.expected
            );
        }
        // The Region a host names, over the label layouts AWS's own hosts put it in.
        let regions: &[(&str, &str, Option<&str>)] = &[
            ("a service endpoint", "s3.us-west-2.amazonaws.com", Some("us-west-2")),
            (
                "a resource's own host",
                "search-things-abc.eu-west-1.es.amazonaws.com",
                Some("eu-west-1"),
            ),
            (
                "a virtual-hosted bucket",
                "bucket.s3.ap-southeast-4.amazonaws.com",
                Some("ap-southeast-4"),
            ),
            ("one naming no Region", "127.0.0.1", None),
            ("a global endpoint", "iam.amazonaws.com", None),
        ];
        for (case, host, expected) in regions {
            let actual = region_of(host);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// WHICH of a handler's own requests addresses a RESOURCE's own published listener, over
    /// a table whose rows span what the rule generalises over: every shape of address the
    /// core publishes such a listener at (a loopback literal with a port, a loopback name, a
    /// bracketed IPv6 literal, an interface address the operator bound), two services' own
    /// listeners on one engine, and the requests that address no published listener at all
    /// (another port of the same host, a listener that has been withdrawn, a host that is a
    /// name, a value that is no URL).
    #[test]
    fn a_published_listener_address_is_addressed_for_the_service_that_published_it() {
        use ::service_common::published_data_planes::PublishedPlanes;

        /// One row: the URL the handler dialled, and the signing name, target and Region the
        /// request is dispatched with.
        struct Row {
            case: &'static str,
            url: &'static str,
            expected: Option<(&'static str, &'static str, &'static str)>,
        }
        let engine = ::engine::InMemoryEngine::new();
        let published = PublishedPlanes::of(&engine).expect("the engine vends the records");
        // The addresses two services opened listeners at, as each published them.
        for (authority, signing_name) in [
            ("127.0.0.1:37333", "aoss"),
            ("localhost:37334", "aoss"),
            ("[::1]:37335", "aoss"),
            ("10.1.2.3:8443", "rds-data"),
            ("127.0.0.1:37336", "aoss"),
        ] {
            published.publish(authority, signing_name);
        }
        // One of them is closed again, so its address stops resolving.
        published.withdraw("127.0.0.1:37336");
        let rows: &[Row] = &[
            Row {
                case: "a collection's own listener on the loopback interface",
                url: "https://127.0.0.1:37333/items/_doc/good-doc",
                expected: Some(("aoss", "/items/_doc/good-doc", "us-west-2")),
            },
            Row {
                case: "a listener on the loopback name, with a query",
                url: "https://localhost:37334/items/_search?size=1",
                expected: Some(("aoss", "/items/_search?size=1", "us-west-2")),
            },
            Row {
                case: "a listener bound to the IPv6 loopback",
                url: "https://[::1]:37335/items",
                expected: Some(("aoss", "/items", "us-west-2")),
            },
            Row {
                case: "another service's listener, addressed with no path at all",
                url: "https://10.1.2.3:8443",
                expected: Some(("rds-data", "/", "us-west-2")),
            },
            Row {
                case: "a listener whose port has been closed again",
                url: "https://127.0.0.1:37336/items",
                expected: None,
            },
            Row {
                case: "another port of a host that has a listener",
                url: "https://127.0.0.1:9297/items",
                expected: None,
            },
            Row {
                case: "a host that is a name nothing was published at",
                url: "https://example.test/items",
                expected: None,
            },
            Row {
                case: "a scheme that is not HTTP",
                url: "ftp://127.0.0.1:37333/items",
                expected: None,
            },
            Row {
                case: "a value that is no URL at all",
                url: "not a url",
                expected: None,
            },
        ];
        for row in rows {
            let case = row.case;
            let addressed = addressed_resource_plane(row.url, Some(&published), "us-west-2");
            let actual = addressed.as_ref().map(|addressed| {
                (
                    addressed.signing_name.as_str(),
                    addressed.target.as_str(),
                    addressed.region.as_str(),
                )
            });
            assert_eq!(
                actual, row.expected,
                "case {case:?}: expected {:?}, got {actual:?}",
                row.expected
            );
        }
        // An emulator vending no such records resolves none of them, which is the position
        // every address was in before a service published one.
        let unrecorded = addressed_resource_plane("https://127.0.0.1:37333/items", None, "us-west-2");
        assert!(
            unrecorded.is_none(),
            "case \"an emulator holding no published addresses\": expected no address, got \
             {unrecorded:?}"
        );
        // The two readings that name a service in the ADDRESS itself answer FIRST, so a plane
        // named in the path keeps its own signing name and an AWS-shaped host keeps its
        // endpoint prefix, whatever has been published.
        for (case, url, expected) in [
            (
                "a plane named in the path of the emulator's own endpoint",
                "https://127.0.0.1:37333/_aws/es/things/index/_doc/1",
                "es",
            ),
            (
                "a host of AWS's own naming",
                "https://s3.us-west-2.amazonaws.com/bucket/key",
                "s3",
            ),
        ] {
            let addressed = addressed(url, "us-west-2")
                .or_else(|| addressed_resource_plane(url, Some(&published), "us-west-2"));
            let actual = addressed
                .as_ref()
                .map(|addressed| addressed.signing_name.as_str());
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: expected the address to name {expected:?}, got {actual:?}"
            );
        }
    }
}
