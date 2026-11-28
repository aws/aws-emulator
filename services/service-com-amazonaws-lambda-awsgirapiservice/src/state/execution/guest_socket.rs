// SPDX-License-Identifier: CC0-1.0

//! The STREAM sockets a running handler opens ITSELF: one connection per socket, carried by
//! the host to a data-plane listener's own protocol handler in this same process.
//!
//! A handler does not reach every data plane over HTTP ([`super::guest_http`]). A database, a
//! cache, a broker or a document store speaks its OWN wire protocol on its own port, and what
//! the control plane publishes for a client to dial is a `host:port`
//! (`DBInstance$Endpoint`, `CacheCluster$ConfigurationEndpoint`, `Cluster$ClusterEndpoint`).
//! A function attached to a VPC reads that address out of the control plane and opens a
//! socket to it, which is the only conforming way to reach a plane AWS documents as reachable
//! from inside the VPC. This module is the host half of that: the guest hands over an address
//! and the host opens the connection, carries the bytes both ways, and — where the protocol
//! runs over TLS — runs the client side of the handshake.
//!
//! What the guest can and cannot do here:
//!
//! * NO SOCKET IS OPENED, on any path. The destination is a data plane this emulator itself
//!   opened, which the service that opened it recorded under the address it published
//!   ([`::service_common::dialable_data_planes`]), or the DNS of the VPC the function is
//!   ATTACHED to, which answers at its port over a connection as well as over a datagram
//!   ([`VpcAttachment::carry_dns_connection`]) — and a connection to either is a PIPE whose
//!   other end that plane's own protocol handler, or that resolver, is serving in this process,
//!   exactly as a request a handler sends over HTTP is carried by the in-process callback. So
//!   the whole of what a run can reach is what a service put in that register and the resolver
//!   of its own VPC, nothing it opens leaves this process, no host port is dialled and no name
//!   is resolved against a host name service — a name is resolved by this environment's own
//!   resolver ([`super::guest_resolver`]) and an address is a KEY here, not something to
//!   connect to. An address neither answers at is answered the way a host answers one — a
//!   refused connection for an address of this machine, no route for an address of another
//!   host, an unresolvable name for a name — and nothing is opened for it.
//! * The guest holds no descriptor and no socket object of the host: what it is handed is a
//!   HANDLE, which is valid for the invocation that opened it and names a connection this
//!   module owns. Every connection an invocation opened is closed when that invocation ends
//!   ([`Sockets::release`]), so no connection outlives the run that opened it and no later
//!   invocation of the same execution environment can read one.
//! * Every bound is the host's: how many connections one invocation may open, how many bytes
//!   it may carry each way, how much one read may return, and how long any one call may wait
//!   — which is clamped to what the RUN has left of its deadline, so no socket call outlasts
//!   the invocation that made it.
//! * The TLS session is REAL: the plane's end runs the SERVER side of the handshake (the
//!   core's own, for a listener the core terminates TLS on) and this seam runs the CLIENT side
//!   with `rustls`, so the version, the cipher suite and the peer certificate the guest reads
//!   back are the ones that handshake negotiated. Nothing of a session is synthesised, and a
//!   handshake that fails is reported as the failure it was.
//!
//! The Python half of the same seam is [`STREAM_SOURCE`], the stream sockets of the `socket`
//! module this runtime supplies ([`super::guest_http::runtime_stdlib_modules`]), and the TLS
//! layer over them is [`super::guest_tls`].

use ::std::collections::BTreeMap;
use ::std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use ::std::time::Duration;

use ::base64::Engine as _;
use ::emulator_server::ListenerConn;
use ::serde_json::{json, Value};
use ::service_common::dialable_data_planes::{DialablePlanes, Dialed};
use ::service_common::vpc_private_dns::{QueryAdmission, PROTOCOL_TCP};
use ::tokio::io::{AsyncReadExt, AsyncWriteExt};
use ::tokio::runtime::Handle;

use super::vpc_network::VpcAttachment;

/// How many connections ONE invocation may hold open at a time.
///
/// A handler opens a session per data plane it talks to, and a few at once is what a client
/// pool holds; a run that asks for more is refused as a host out of descriptors refuses one.
const MAX_OPEN_CONNECTIONS: usize = 8;

/// How many connections ONE invocation may open in total, held where no guest code can reach
/// it: a run cannot open connections without end, however many it closes.
const MAX_CONNECTIONS_PER_RUN: usize = 64;

/// How many bytes ONE invocation may carry in each direction, across every connection it
/// opened. The same ceiling as the answer one AWS SDK call may read back
/// ([`super::aws_api::MAX_RESPONSE_BYTES`]), because it bounds the same thing: how much of
/// this process's memory one run can take by asking for data.
const MAX_BYTES_EACH_WAY: usize = 8 * 1024 * 1024;

/// How much ONE read may return, and how much ONE write may carry.
const MAX_BYTES_PER_CALL: usize = 1024 * 1024;

/// How many DATAGRAMS one invocation may send, across every socket it opened.
///
/// A datagram is not a connection — nothing is held open for one — so what has to be bounded
/// is how many a run may hand over: a client resolves a name and sends a measurement or two,
/// and a run that asks for more than this is answered as a host out of descriptors answers
/// one.
const MAX_DATAGRAMS_PER_RUN: usize = 256;

/// How many bytes ONE datagram may carry: the largest payload an IPv4 UDP datagram can hold
/// (65535 less the 20-byte IP and 8-byte UDP headers), so a datagram this seam refuses is one
/// no host could have sent either.
const MAX_DATAGRAM_BYTES: usize = 65_507;

/// The ports a host LENDS a socket that bound none itself: Linux's own default ephemeral
/// range (`net.ipv4.ip_local_port_range`, 32768-60999), which is what a connected socket
/// reports as its own port.
const EPHEMERAL_PORT_FIRST: u16 = 32768;
/// See [`EPHEMERAL_PORT_FIRST`].
const EPHEMERAL_PORT_LAST: u16 = 60999;

/// How long a connect or a read waits when the guest asked for no timeout at all: a socket
/// call still cannot wait without end, and what is left of the run's deadline bounds it
/// besides ([`clamped`]).
const WAIT_CEILING: Duration = Duration::from_secs(30);

/// How many bytes may be in flight on each half of the pipe a query to the VPC's own DNS runs
/// on ([`dns_connection`]): one whole DNS message and the two-byte length prefix framing it
/// (RFC 1035 §4.2.2), which is the largest message that prefix can count — so neither end has
/// to wait on the other inside one message.
const DNS_PIPE_BYTES: usize = u16::MAX as usize + 2;

// The numbers a host answers these failures with, as a Python program reads them off
// `errno`: what a client branches on to tell a refused connection from a reset one.
const EBADF: i32 = 9;
const EMFILE: i32 = 24;
const ENETUNREACH: i32 = 101;
const ECONNRESET: i32 = 104;
const ENOTCONN: i32 = 107;
const ETIMEDOUT: i32 = 110;
const ECONNREFUSED: i32 = 111;
const EPIPE: i32 = 32;
/// `EAI_NONAME`, which is what a resolver answers for a name it holds nothing for.
const EAI_NONAME: i32 = -2;

/// The connections ONE execution environment holds, and what the invocation being served has
/// spent of the bounds above.
///
/// Held by the environment's session ([`super::python`]), so the host functions of that
/// environment reach the connections of the invocation they are serving and of no other: the
/// whole set is dropped when an invocation ends, which closes every socket it opened.
#[derive(Default)]
pub struct Sockets {
    held: Mutex<Open>,
}

/// Written by hand rather than derived: a connection is a live socket and carries no `Debug`.
/// What is diagnosable here is how many are open — never a peer address in a log line.
impl ::std::fmt::Debug for Sockets {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("Sockets")
            .field("open", &self.locked().connections.len())
            .finish()
    }
}

/// What one invocation has open and what it has spent.
#[derive(Default)]
struct Open {
    /// The connections it holds, by the handle the guest was given.
    connections: BTreeMap<u64, Connection>,
    /// The last handle handed out. Handles are never reused inside one invocation, so a
    /// handle of a closed connection names nothing rather than another connection.
    handed: u64,
    /// How many connections this invocation has opened.
    opened: usize,
    /// How many bytes it has written, and how many it has read.
    written: usize,
    read: usize,
    /// How many datagrams it has sent.
    datagrams: usize,
    /// How many ephemeral ports have been lent to its sockets, which is what the next one is
    /// counted from.
    lent_ports: u16,
}

/// ONE connection the host holds for the guest: the CLIENT end of a pipe whose other end the
/// plane's own protocol handler is serving ([`::service_common::dialable_data_planes`]).
struct Connection {
    /// The byte stream itself — with a negotiated TLS session over it once there is one, and
    /// `None` for a connection a handshake could not finish, which is a connection that is
    /// gone.
    wire: Option<ListenerConn>,
    /// Whether a TLS session is negotiated on it.
    secured: bool,
    /// The task answering the OTHER end, for a connection this seam itself serves — the DNS of
    /// the function's own VPC ([`dns_connection`]) — and `None` for a connection whose other end
    /// a data plane's own handler serves. It is ABORTED when the connection is dropped
    /// ([`Connection::drop`]), so no task of a closed connection, or of an invocation that has
    /// ended, is left running.
    answering: Option<::tokio::task::JoinHandle<()>>,
}

/// Dropping a connection ends BOTH halves of it: the client end goes, which is what a plane's
/// own handler reads as the end of file, and a task this seam spawned to answer the other end is
/// stopped rather than left to notice.
impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(answering) = self.answering.take() {
            answering.abort();
        }
    }
}

impl Connection {
    /// Read what has arrived, up to `buffer`'s length, waiting no longer than `waiting`.
    ///
    /// Zero bytes is the plane's end of file, which is what a client of a closed connection
    /// reads.
    fn read(
        &mut self,
        runtime: &Handle,
        buffer: &mut [u8],
        waiting: Duration,
    ) -> Result<usize, Failure> {
        let wire = self.wire.as_mut().ok_or_else(Failure::gone)?;
        runtime.block_on(async {
            match ::tokio::time::timeout(waiting, wire.read(buffer)).await {
                Ok(Ok(read)) => Ok(read),
                Ok(Err(error)) => Err(Failure::of_io(&error)),
                Err(_elapsed) => Err(Failure::timed_out()),
            }
        })
    }

    /// Write `payload` whole, waiting no longer than `waiting`.
    fn write(
        &mut self,
        runtime: &Handle,
        payload: &[u8],
        waiting: Duration,
    ) -> Result<(), Failure> {
        let wire = self.wire.as_mut().ok_or_else(Failure::gone)?;
        runtime.block_on(async {
            let written = async {
                wire.write_all(payload).await?;
                wire.flush().await
            };
            match ::tokio::time::timeout(waiting, written).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(Failure::of_io(&error)),
                Err(_elapsed) => Err(Failure::timed_out()),
            }
        })
    }

    /// Stop WRITING on this connection, which is the half a client can close: the plane reads
    /// the end of file the protocol frames its last message with. Closing only the reading
    /// half (`SHUT_RD`) stops nothing on a pipe, so it is answered without doing anything, and
    /// reading after it still answers what the plane wrote.
    fn shutdown(&mut self, runtime: &Handle, how: i64, waiting: Duration) -> Result<(), Failure> {
        if how == 0 {
            return Ok(());
        }
        let wire = self.wire.as_mut().ok_or_else(Failure::gone)?;
        runtime.block_on(async {
            match ::tokio::time::timeout(waiting, wire.shutdown()).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(Failure::of_io(&error)),
                Err(_elapsed) => Err(Failure::timed_out()),
            }
        })
    }

    /// Run the CLIENT side of a TLS handshake on this connection, and report what it
    /// negotiated.
    ///
    /// The handshake is real: the plane's own end runs the SERVER side of it — the core's, for
    /// a listener the core terminates TLS on — so a certificate that does not verify or a
    /// client the server refuses fails HERE. The certificate is verified against the one this
    /// emulator's core presents on every listener it opens ([`client_config`]), under a name it
    /// was issued for ([`VERIFIED_NAME`]).
    fn negotiate(&mut self, runtime: &Handle, waiting: Duration) -> Result<Value, Failure> {
        if self.secured {
            return Err(Failure::tls(
                "a TLS session is already negotiated on this connection".to_string(),
            ));
        }
        let wire = self.wire.take().ok_or_else(Failure::gone)?;
        let config = client_config()?;
        let name = ::rustls::pki_types::ServerName::try_from(VERIFIED_NAME)
            .map_err(|error| Failure::tls(error.to_string()))?;
        let connector = ::tokio_rustls::TlsConnector::from(config);
        let negotiated = runtime.block_on(async {
            match ::tokio::time::timeout(waiting, connector.connect(name, wire)).await {
                Ok(Ok(secured)) => Ok(secured),
                Ok(Err(error)) => Err(Failure::tls(error.to_string())),
                Err(_elapsed) => Err(Failure::timed_out()),
            }
        })?;
        let session = negotiated_session(negotiated.get_ref().1);
        self.wire = Some(Box::new(negotiated));
        self.secured = true;
        Ok(session)
    }

    /// Close the TLS session and leave the connection in the clear, as `SSLSocket.unwrap`
    /// does: the close notification is sent, and the stream underneath stays open.
    fn release(&mut self, runtime: &Handle, waiting: Duration) -> Result<(), Failure> {
        if !self.secured {
            return Err(Failure::tls(
                "this connection has no TLS session to release".to_string(),
            ));
        }
        let wire = self.wire.take().ok_or_else(Failure::gone)?;
        // The stream is the TLS one, so what is taken back out of it is the pipe it ran on.
        // Downcasting it is not open to this seam, so the session is ended by writing its
        // close notification and the connection keeps the TLS stream, which carries plain
        // bytes no further: a client that unwraps a session stops using it, which is what the
        // standard library's own `unwrap` is for.
        let mut wire = wire;
        let notified = runtime.block_on(async {
            match ::tokio::time::timeout(waiting, wire.shutdown()).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(Failure::of_io(&error)),
                Err(_elapsed) => Err(Failure::timed_out()),
            }
        });
        self.wire = Some(wire);
        self.secured = false;
        notified
    }
}

/// What one call of this seam came to when it could not be served: the error a host answers
/// it with, as the guest raises it.
struct Failure {
    /// WHICH failure it is, which decides the exception the guest raises: a refused
    /// connection, a timeout, a reset peer, a name that resolves to nothing, a TLS failure,
    /// or a bound of this seam.
    kind: &'static str,
    /// The number a program reads off `errno`, where the failure has one.
    errno: Option<i32>,
    /// What it says.
    message: String,
}

impl Failure {
    /// A connection refused: this emulator has opened no plane at the address, or the plane
    /// behind it is gone. The wording a host answers one with, which carries no address of its
    /// own.
    fn refused() -> Self {
        Self {
            kind: "refused",
            errno: Some(ECONNREFUSED),
            message: "Connection refused".to_string(),
        }
    }

    /// An address with no route to it: nothing of this emulator answers there, and it is not
    /// an address of this machine either, so there is nowhere for a connection to go.
    fn no_route() -> Self {
        Self {
            kind: "other",
            errno: Some(ENETUNREACH),
            message: "Network is unreachable".to_string(),
        }
    }

    /// A wait that was not answered inside what the call was allowed.
    fn timed_out() -> Self {
        Self {
            kind: "timeout",
            errno: Some(ETIMEDOUT),
            message: "timed out".to_string(),
        }
    }

    /// A name nothing resolves: no resolver of this runtime holds an address for it.
    ///
    /// The wording a host answers one with, which names neither the name asked about nor this
    /// emulator: it is what `gai_strerror` states for `EAI_NONAME`, so a program reading the
    /// error reads what it reads off a host.
    fn unresolved() -> Self {
        Self {
            kind: "name",
            errno: Some(EAI_NONAME),
            message: "Name or service not known".to_string(),
        }
    }

    /// A handle that names no connection of the invocation making the call.
    fn not_connected() -> Self {
        Self {
            kind: "other",
            errno: Some(ENOTCONN),
            message: "Transport endpoint is not connected".to_string(),
        }
    }

    /// A connection a handshake left behind, which nothing can be carried on.
    fn gone() -> Self {
        Self {
            kind: "other",
            errno: Some(EBADF),
            message: "Bad file descriptor".to_string(),
        }
    }

    /// A TLS failure: the handshake, or the session over it.
    fn tls(message: String) -> Self {
        Self {
            kind: "tls",
            errno: None,
            message,
        }
    }

    /// A bound of this seam, which is the host's and not a host network error.
    fn bounded(message: String) -> Self {
        Self {
            kind: "other",
            errno: Some(EMFILE),
            message,
        }
    }

    /// The call itself was not one this seam makes.
    fn not_made(message: String) -> Self {
        Self {
            kind: "other",
            errno: None,
            message,
        }
    }

    /// What the host answered, as the error a program branches on.
    fn of_io(error: &::std::io::Error) -> Self {
        use ::std::io::ErrorKind;

        let (kind, errno) = match error.kind() {
            ErrorKind::WouldBlock | ErrorKind::TimedOut => ("timeout", Some(ETIMEDOUT)),
            ErrorKind::ConnectionRefused => ("refused", Some(ECONNREFUSED)),
            ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted => {
                ("reset", Some(ECONNRESET))
            }
            ErrorKind::BrokenPipe => ("other", Some(EPIPE)),
            ErrorKind::NotConnected => ("other", Some(ENOTCONN)),
            _ => ("other", error.raw_os_error().or(Some(ENETUNREACH))),
        };
        Self {
            kind,
            errno,
            message: error.to_string(),
        }
    }

    /// The document the guest reads it back as.
    fn document(&self) -> Value {
        json!({
            "error": {
                "kind": self.kind,
                "errno": self.errno,
                "message": self.message,
            }
        })
    }
}

impl Sockets {
    /// Answer ONE call the guest made on this seam: `verb` with `document`, within `within` —
    /// what the run has left of its deadline, which bounds every wait this call makes.
    ///
    /// `dialing` is the register of the planes this emulator has opened and the addresses they
    /// answer at; `None` where the engine vends none, and then no address answers at all,
    /// which is the position every address was in before a service opened a plane. `inside` is
    /// the VPC this invocation's function is ATTACHED to ([`super::vpc_network`]), and `None`
    /// for a function attached to none — which then has no route to a VPC address at all.
    /// `runtime` is the async runtime the plane's own handler runs on, which this blocking call
    /// drives the connection on.
    pub fn answered(
        &self,
        dialing: Option<&DialablePlanes>,
        inside: Option<&VpcAttachment>,
        runtime: &Handle,
        verb: &str,
        document: &str,
        within: Duration,
    ) -> String {
        let answered = match ::serde_json::from_str::<Value>(document) {
            Ok(asked) => match verb {
                "connect" => self.connected(dialing, inside, runtime, &asked, within),
                "resolve" => resolved_name(inside, &asked),
                "route" => self.routed(inside, &asked),
                "datagram" => self.delivered(inside, &asked),
                "send" => self.sent(runtime, &asked, within),
                "recv" => self.received(runtime, &asked, within),
                "close" => self.closed(&asked),
                "shutdown" => self.shut(runtime, &asked, within),
                "tls" => self.secured(runtime, &asked, within),
                "untls" => self.unsecured(runtime, &asked, within),
                other => Err(Failure::not_made(format!(
                    "{other} is not a call a handler makes on a socket"
                ))),
            },
            Err(error) => Err(Failure::not_made(format!(
                "the socket call carried no document: {error}"
            ))),
        };
        match answered {
            Ok(answer) => answer.to_string(),
            Err(failed) => failed.document().to_string(),
        }
    }

    /// Close every connection the invocation just served opened, and give it back its whole
    /// byte and connection budget.
    ///
    /// Dropping a connection drops the client end of its pipe, which ends the plane's own
    /// side of it, so nothing an invocation opened stays open between invocations of one
    /// execution environment.
    pub fn release(&self) {
        let mut held = self.locked();
        held.connections.clear();
        held.handed = 0;
        held.opened = 0;
        held.written = 0;
        held.read = 0;
        held.datagrams = 0;
        held.lent_ports = 0;
    }

    /// How many connections are open right now, for the tests of this seam.
    #[must_use]
    pub fn open(&self) -> usize {
        self.locked().connections.len()
    }

    /// Open ONE connection to the address the guest named.
    fn connected(
        &self,
        dialing: Option<&DialablePlanes>,
        inside: Option<&VpcAttachment>,
        runtime: &Handle,
        asked: &Value,
        within: Duration,
    ) -> Result<Value, Failure> {
        let host = asked
            .get("host")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let port = port_of(asked)?;
        // The bounds first: a run past them opens no connection at all.
        {
            let held = self.locked();
            if held.connections.len() >= MAX_OPEN_CONNECTIONS {
                return Err(Failure::bounded(format!(
                    "one invocation may hold {MAX_OPEN_CONNECTIONS} connections open at a time"
                )));
            }
            if held.opened >= MAX_CONNECTIONS_PER_RUN {
                return Err(Failure::bounded(format!(
                    "one invocation may open {MAX_CONNECTIONS_PER_RUN} connections"
                )));
            }
        }
        // WHERE it may go: a plane this emulator itself opened, recorded under the address it
        // published — there is nothing else such a connection COULD go to, because what is
        // opened is a pipe to that plane's handler — or the DNS of the function's own VPC,
        // which answers a query carried on a connection to its port as well as one carried in
        // a datagram ([`dns_connection`]). An address neither answers at is answered as a host
        // answers one.
        let dialed = dialing.and_then(|dialing| dialing.dialable(&host, port));
        let waiting = clamped(asked, within);
        // Where the connection LEAVES from, read off the same route a datagram's is
        // ([`Self::source_toward`]): the private address of the interface the function's VPC
        // attachment holds for an address of that VPC, and this machine's own interface for a
        // plane served in this process. A destination the attachment has no source for leaves
        // the socket bound to nothing, which is what an unbound socket reports. Read BEFORE
        // the connections are locked, because the route lends a port out of the same state.
        let source = self.source_toward(inside, &host).ok();
        // WHO the plane is serving, as the plane's own protocol states its client: a data
        // plane of this emulator is published at an address of THIS MACHINE, so the transport
        // is a pipe inside the process — but the client is the FUNCTION, which reaches the
        // VPC the plane was placed in through the interfaces Lambda stood up for it, and the
        // address its traffic carries there is the private address of one of them.
        //
        // So the two readings of ONE socket differ on purpose, and each is right about its
        // own side: `sourceHost` below is where the GUEST's traffic toward this destination
        // leaves from — this machine's own interface for a plane served in this process — and
        // what the plane is told is where the function sits in the VPC the plane belongs to,
        // which is the address its protocol would state for this client. The PORT is the one
        // the socket was lent, so both readings name the same socket.
        //
        // A function attached to NO VPC has no address there, and the connection still leaves
        // from somewhere: the interface of this machine the route lookup answered with, which
        // is the address the socket's own `getsockname` reports to the guest. That is what the
        // plane is told then — so every connection this seam opens states where it came from,
        // and no plane that names its client has to invent one.
        let from = inside
            .and_then(VpcAttachment::private_address)
            .map(|address| {
                ::std::net::SocketAddr::new(
                    ::std::net::IpAddr::V4(address),
                    source
                        .as_ref()
                        .map_or_else(|| self.lent_port(), |(_address, port)| *port),
                )
            })
            .or_else(|| {
                let (address, port) = source.as_ref()?;
                Some(::std::net::SocketAddr::new(address.parse().ok()?, *port))
            });
        let (authority, wire, answering) = match dialed {
            Some(Dialed { authority, plane, .. }) => {
                let wire = runtime.block_on(async {
                    match ::tokio::time::timeout(waiting, plane.dial(from)).await {
                        Ok(Ok(wire)) => Ok(wire),
                        Ok(Err(error)) => Err(Failure::of_io(&error)),
                        Err(_elapsed) => Err(Failure::timed_out()),
                    }
                })?;
                (authority, wire, None)
            }
            None => match dns_connection(inside, &host, port, runtime) {
                Some(answered) => {
                    let (authority, wire, answering) = answered?;
                    (authority, wire, Some(answering))
                }
                None => return Err(nothing_answers(inside, &host)),
            },
        };
        let mut held = self.locked();
        held.handed += 1;
        held.opened += 1;
        let handle = held.handed;
        held.connections.insert(
            handle,
            Connection {
                wire: Some(wire),
                secured: false,
                answering,
            },
        );
        let (peer_host, peer_port) = peer_of(&authority, &host, port);
        Ok(json!({
            "handle": handle,
            "peerHost": peer_host,
            "peerPort": peer_port,
            "dialed": authority,
            "sourceHost": source.as_ref().map(|(address, _)| address.clone()),
            "sourcePort": source.as_ref().map(|(_, port)| *port),
        }))
    }

    /// The address a datagram or a connection toward the address this call names LEAVES from:
    /// the route lookup a host performs before it sends one.
    fn routed(&self, inside: Option<&VpcAttachment>, asked: &Value) -> Result<Value, Failure> {
        let host = host_of(asked);
        // A route is looked up toward an address AND a port, as a host's own lookup is: the
        // port decides nothing about which interface answers, but a call that names none is
        // not one this seam makes.
        port_of(asked)?;
        let (source, source_port) = self.source_toward(inside, &host)?;
        Ok(json!({ "source": source, "sourcePort": source_port }))
    }

    /// Deliver ONE datagram to the address it is addressed to, and answer what came back.
    ///
    /// The addresses of the VPC this emulator answers a datagram at are the ones its DNS is
    /// served at ([`super::vpc_network::VpcAttachment::dns_reply`]): the VPC's own resolver, and
    /// an inbound Route 53 Resolver endpoint's addresses while the security groups on them admit
    /// the query. Every other address of the VPC takes the datagram and answers nothing, as a
    /// host with nothing bound to that port does — UDP reports nothing back, so the datagram
    /// is simply lost — and so does one a security group does not admit; an address the
    /// attachment has no route to is answered the way a host with no route to it answers one,
    /// with nothing sent.
    ///
    /// An address of THIS MACHINE is the one case a datagram is REFUSED: the only datagram
    /// service this emulator runs is the in-process X-Ray daemon, which a handler delivers to
    /// without reaching this seam at all, so every other port of this machine has nothing
    /// bound to it — and a datagram to an unbound port of a reachable host is refused, which
    /// is what the sender reads off the port-unreachable the host answers with.
    fn delivered(
        &self,
        inside: Option<&VpcAttachment>,
        asked: &Value,
    ) -> Result<Value, Failure> {
        let host = host_of(asked);
        let port = port_of(asked)?;
        let payload = payload_of(asked)?;
        if payload.len() > MAX_DATAGRAM_BYTES {
            return Err(Failure::not_made(format!(
                "one datagram may carry {MAX_DATAGRAM_BYTES} bytes"
            )));
        }
        // The route first: a datagram toward an address with no route is never handed over.
        let (source, source_port) = self.source_toward(inside, &host)?;
        if this_machine(&host) {
            return Err(Failure::refused());
        }
        {
            let mut held = self.locked();
            if held.datagrams >= MAX_DATAGRAMS_PER_RUN {
                return Err(Failure::bounded(format!(
                    "one invocation may send {MAX_DATAGRAMS_PER_RUN} datagrams"
                )));
            }
            held.datagrams += 1;
        }
        let answered = host
            .parse::<::std::net::Ipv4Addr>()
            .ok()
            .zip(inside)
            .and_then(|(destination, inside)| inside.dns_reply(destination, port, &payload))
            .map(|reply| ::base64::engine::general_purpose::STANDARD.encode(reply));
        Ok(json!({
            "source": source,
            "sourcePort": source_port,
            "data": answered,
        }))
    }

    /// The address traffic toward `host`:`port` leaves from, and the ephemeral port the socket
    /// is lent for it.
    ///
    /// This machine's own interface is where every plane this emulator opened and the
    /// in-process X-Ray daemon are served, so traffic to it leaves from that same interface.
    /// Everything else is reached through the function's VPC attachment, which has a route to
    /// the addresses of its own VPC and to nothing else; a value that is a NAME rather than an
    /// address is answered by what resolves it, which is nothing but this machine's own names.
    fn source_toward(
        &self,
        inside: Option<&VpcAttachment>,
        host: &str,
    ) -> Result<(String, u16), Failure> {
        // WHICH address the destination is, as the environment's own resolver answers it
        // ([`super::guest_resolver`]): an address is itself, and a name is what that resolver
        // holds for it — so a route is looked up toward the same address a handler resolving
        // the name itself would then address.
        let address = *super::guest_resolver::addresses_of(inside, host)
            .first()
            .ok_or_else(Failure::unresolved)?;
        if address.is_loopback() || bound_to(address) {
            return Ok((address.to_string(), self.lent_port()));
        }
        let ::std::net::IpAddr::V4(destination) = address else {
            return Err(nothing_answers(inside, host));
        };
        let source = inside
            .and_then(|inside| inside.source_toward(destination))
            .ok_or_else(|| nothing_answers(inside, host))?;
        Ok((source.to_string(), self.lent_port()))
    }

    /// The next ephemeral port this invocation's sockets are lent, out of the range a host
    /// lends from ([`EPHEMERAL_PORT_FIRST`]).
    fn lent_port(&self) -> u16 {
        let mut held = self.locked();
        let lent = held.lent_ports % (EPHEMERAL_PORT_LAST - EPHEMERAL_PORT_FIRST + 1);
        held.lent_ports = lent.wrapping_add(1);
        EPHEMERAL_PORT_FIRST + lent
    }

    /// Write what the guest handed over, whole.
    fn sent(&self, runtime: &Handle, asked: &Value, within: Duration) -> Result<Value, Failure> {
        let handle = handle_of(asked)?;
        let payload = payload_of(asked)?;
        if payload.len() > MAX_BYTES_PER_CALL {
            return Err(Failure::bounded(format!(
                "one write may carry {MAX_BYTES_PER_CALL} bytes"
            )));
        }
        let waiting = clamped(asked, within);
        let mut held = self.locked();
        let Open {
            connections,
            written,
            ..
        } = &mut *held;
        if *written + payload.len() > MAX_BYTES_EACH_WAY {
            return Err(Failure::bounded(format!(
                "one invocation may write {MAX_BYTES_EACH_WAY} bytes to its connections"
            )));
        }
        let connection = connections
            .get_mut(&handle)
            .ok_or_else(Failure::not_connected)?;
        connection.write(runtime, &payload, waiting)?;
        *written += payload.len();
        Ok(json!({ "sent": payload.len() }))
    }

    /// Read what has arrived on one connection.
    fn received(
        &self,
        runtime: &Handle,
        asked: &Value,
        within: Duration,
    ) -> Result<Value, Failure> {
        let handle = handle_of(asked)?;
        let size = asked
            .get("size")
            .and_then(Value::as_u64)
            .map_or(0, |size| usize::try_from(size).unwrap_or(MAX_BYTES_PER_CALL))
            .min(MAX_BYTES_PER_CALL);
        let waiting = clamped(asked, within);
        let mut held = self.locked();
        let Open {
            connections, read, ..
        } = &mut *held;
        if *read >= MAX_BYTES_EACH_WAY {
            return Err(Failure::bounded(format!(
                "one invocation may read {MAX_BYTES_EACH_WAY} bytes from its connections"
            )));
        }
        let connection = connections
            .get_mut(&handle)
            .ok_or_else(Failure::not_connected)?;
        // Never past the ceiling: what is left of the invocation's read budget bounds this
        // read, as what is left of its write budget bounds a write.
        let size = size.min(MAX_BYTES_EACH_WAY - *read);
        let mut buffer = vec![0_u8; size];
        let arrived = connection.read(runtime, &mut buffer, waiting)?;
        *read += arrived;
        Ok(json!({
            "data": ::base64::engine::general_purpose::STANDARD.encode(&buffer[..arrived]),
        }))
    }

    /// Close one connection, which ends the plane's own side of it.
    fn closed(&self, asked: &Value) -> Result<Value, Failure> {
        let handle = handle_of(asked)?;
        let closed = self.locked().connections.remove(&handle).is_some();
        Ok(json!({ "closed": closed }))
    }

    /// Stop writing on one connection.
    fn shut(&self, runtime: &Handle, asked: &Value, within: Duration) -> Result<Value, Failure> {
        let handle = handle_of(asked)?;
        let how = asked.get("how").and_then(Value::as_i64).unwrap_or(2);
        let waiting = clamped(asked, within);
        let mut held = self.locked();
        held.connections
            .get_mut(&handle)
            .ok_or_else(Failure::not_connected)?
            .shutdown(runtime, how, waiting)?;
        Ok(json!({ "shutdown": how }))
    }

    /// Negotiate a TLS session on one connection.
    fn secured(
        &self,
        runtime: &Handle,
        asked: &Value,
        within: Duration,
    ) -> Result<Value, Failure> {
        let handle = handle_of(asked)?;
        let waiting = clamped(asked, within);
        let mut held = self.locked();
        held.connections
            .get_mut(&handle)
            .ok_or_else(Failure::not_connected)?
            .negotiate(runtime, waiting)
    }

    /// Close the TLS session on one connection.
    fn unsecured(
        &self,
        runtime: &Handle,
        asked: &Value,
        within: Duration,
    ) -> Result<Value, Failure> {
        let handle = handle_of(asked)?;
        let waiting = clamped(asked, within);
        let mut held = self.locked();
        held.connections
            .get_mut(&handle)
            .ok_or_else(Failure::not_connected)?
            .release(runtime, waiting)?;
        Ok(json!({ "released": true }))
    }

    /// The connections, under the lock that is never held while another is taken.
    fn locked(&self) -> MutexGuard<'_, Open> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The connection a query carried to the DNS of the function's OWN VPC runs on: the client end
/// of a pipe whose other end that VPC's resolver answers on
/// ([`VpcAttachment::carry_dns_connection`]).
///
/// `None` when the address this call names is not one the function's VPC answers DNS at, which
/// leaves the connection to be answered the way a host answers one with nothing there. An `Err`
/// when it IS such an address but no rule of the interface it sits behind admits a connection to
/// it from this function: the packet that would open the connection is dropped, so the client
/// waits for an answer that never comes — the same stop a dropped datagram comes to.
///
/// The address is read the way every other destination of this seam is: through the resolver of
/// this execution environment ([`super::guest_resolver`]), so a connection is opened toward the
/// same address a handler resolving the name itself would then address.
fn dns_connection(
    inside: Option<&VpcAttachment>,
    host: &str,
    port: u16,
    runtime: &Handle,
) -> Option<Result<ServedConnection, Failure>> {
    let inside = inside?;
    let ::std::net::IpAddr::V4(destination) =
        super::guest_resolver::addresses_of(Some(inside), host)
            .into_iter()
            .next()?
    else {
        return None;
    };
    match inside.dns_admission(destination, port, PROTOCOL_TCP) {
        QueryAdmission::NoServer => None,
        QueryAdmission::Dropped => Some(Err(Failure::timed_out())),
        QueryAdmission::Admitted => {
            let (client_end, server_end) = ::tokio::io::duplex(DNS_PIPE_BYTES);
            let answering = inside.clone();
            // The resolver's own end, answering for as long as the client holds the connection
            // open. It is stopped when that connection is dropped — which closing it and the
            // invocation's own release both do ([`Sockets::release`], [`Connection::drop`]) — so
            // nothing of it outlives the run, and it does work only when the client writes.
            let serving = runtime.spawn(async move {
                let mut server_end = server_end;
                answering.carry_dns_connection(&mut server_end).await;
            });
            Some(Ok((
                format!("{destination}:{port}"),
                Box::new(client_end),
                serving,
            )))
        }
    }
}

/// A connection this seam serves the OTHER end of itself ([`dns_connection`]): the authority it
/// is reported to the guest under, the client end the guest is handed, and the task answering
/// the other end.
type ServedConnection = (String, ListenerConn, ::tokio::task::JoinHandle<()>);

/// What a connection to an address NO plane of this emulator answers at comes to, as a host
/// answers it, read off the address the destination RESOLVES to
/// ([`super::guest_resolver`]): a refused connection for an address of THIS machine — the
/// address is here and nothing is listening on that port — no route for an address of another
/// host, and a name that resolves to nothing for a name this environment's resolver holds no
/// address under.
fn nothing_answers(inside: Option<&VpcAttachment>, host: &str) -> Failure {
    let Some(address) = super::guest_resolver::addresses_of(inside, host)
        .into_iter()
        .next()
    else {
        return Failure::unresolved();
    };
    if address.is_loopback() || bound_to(address) {
        return Failure::refused();
    }
    Failure::no_route()
}

/// The addresses a NAME has, as the resolver of this execution environment answers it
/// ([`super::guest_resolver`]) — one entry per address, naming the family it belongs to, in
/// the order the resolver answers them, beside the CANONICAL name the lookup ended at and the
/// aliases it passed through on the way, which is the rest of what a lookup asking for the
/// whole answer reports.
///
/// A name that resolver holds no address under is answered as a name that does not exist,
/// which is what the guest raises for one.
fn resolved_name(inside: Option<&VpcAttachment>, asked: &Value) -> Result<Value, Failure> {
    let host = host_of(asked);
    let resolved = super::guest_resolver::resolution_of(inside, &host);
    if resolved.addresses.is_empty() {
        return Err(Failure::unresolved());
    }
    let answered: Vec<Value> = resolved
        .addresses
        .iter()
        .map(|address| {
            json!({
                "address": address.to_string(),
                "family": if address.is_ipv4() { "inet" } else { "inet6" },
            })
        })
        .collect();
    Ok(json!({
        "addresses": answered,
        "canonical": resolved.canonical,
        "aliases": resolved.aliases,
    }))
}

/// Whether `address` is the interface the core binds its listeners on, as the operator chose
/// it.
///
/// A CONCRETE address only: a wildcard bind names every interface of this machine and so
/// names no particular address a plane could be answering for, which leaves loopback — the
/// address the core itself publishes a wildcard-bound listener at — as the one address of this
/// machine either way.
fn bound_to(address: ::std::net::IpAddr) -> bool {
    ::emulator_server::bind_address_from_env()
        .is_ok_and(|bound| !bound.is_unspecified() && bound == address)
}

/// Whether `host` names an address of THIS machine: one of its loopback addresses, one of its
/// loopback names, or the interface the core binds its listeners on.
fn this_machine(host: &str) -> bool {
    let named = host.trim_start_matches('[').trim_end_matches(']');
    match named.parse::<::std::net::IpAddr>() {
        Ok(address) => address.is_loopback() || bound_to(address),
        Err(_name) => ::emulator_server::is_loopback_host(named),
    }
}

/// The address one connection reports as its peer: the host and port the register holds the
/// plane under, and the ones the client addressed where that authority spells no port.
fn peer_of(authority: &str, host: &str, port: u16) -> (String, u16) {
    match authority.rsplit_once(':') {
        Some((held_host, held_port)) => (
            held_host.trim_start_matches('[').trim_end_matches(']').to_string(),
            held_port.parse().unwrap_or(port),
        ),
        None => (host.to_string(), port),
    }
}

/// The HOST one call names, as the guest spelled it.
fn host_of(asked: &Value) -> String {
    asked
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The handle one call names, or the failure a call naming no connection answers.
fn handle_of(asked: &Value) -> Result<u64, Failure> {
    asked
        .get("handle")
        .and_then(Value::as_u64)
        .ok_or_else(Failure::not_connected)
}

/// The port one call names.
fn port_of(asked: &Value) -> Result<u16, Failure> {
    asked
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .ok_or_else(|| Failure::not_made("a connection is opened to a port".to_string()))
}

/// The bytes one write carries, which travel base64-encoded because this seam is text.
fn payload_of(asked: &Value) -> Result<Vec<u8>, Failure> {
    let data = asked.get("data").and_then(Value::as_str).unwrap_or_default();
    ::base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| Failure::not_made(format!("the bytes to write are not base64: {error}")))
}

/// How long ONE call may wait: what the guest asked for, bounded by what the run has left of
/// its deadline and by the ceiling a call with no timeout of its own gets.
///
/// The guest's own value is a float it chose, so it is CLAMPED BEFORE it is made a span: a
/// magnitude no `Duration` can hold is the ceiling, like any other value past it, rather than
/// a failure of the conversion. Never zero either: a call whose run has no time left is
/// answered by the run's own deadline ([`super::python`]) rather than by a wait that ends
/// instantly.
fn clamped(asked: &Value, within: Duration) -> Duration {
    let ceiling = WAIT_CEILING.as_secs_f64();
    let asked = asked
        .get("timeout")
        .and_then(Value::as_f64)
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(|seconds| Duration::from_secs_f64(seconds.min(ceiling)))
        .unwrap_or(WAIT_CEILING);
    let bounded = asked.min(WAIT_CEILING).min(within);
    bounded.max(Duration::from_millis(1))
}

/// The name a session this seam negotiates is verified under.
///
/// The certificate this emulator's core presents on every listener it opens is issued for this
/// machine's own names (`emulator_server::TlsCertificateAuthority`), and the plane at the other
/// end of the connection is served in THIS process — so the name verified is the one of those
/// names every such plane answers under. The `server_hostname` a guest names is reported back
/// to it beside the session and decides nothing here, because a name AWS would resolve to a
/// data plane names no plane of this emulator.
const VERIFIED_NAME: &str = "localhost";

/// The client configuration every session this seam negotiates is built from, made once for
/// this process.
///
/// It trusts exactly ONE certificate: the one this emulator's core presents on every listener
/// it opens (`emulator_server::TlsCertificateAuthority`). So a session is negotiated with a
/// listener of this emulator and with nothing else, and the verification a client performs
/// against a certificate authority really is performed.
fn client_config() -> Result<::std::sync::Arc<::rustls::ClientConfig>, Failure> {
    static HELD: OnceLock<Result<::std::sync::Arc<::rustls::ClientConfig>, String>> =
        OnceLock::new();
    HELD.get_or_init(|| {
        let provider = ::std::sync::Arc::new(::rustls::crypto::aws_lc_rs::default_provider());
        let mut roots = ::rustls::RootCertStore::empty();
        roots
            .add(
                ::emulator_server::TlsCertificateAuthority::for_this_process()
                    .der()
                    .clone(),
            )
            .map_err(|error| error.to_string())?;
        let config = ::rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|error| error.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(::std::sync::Arc::new(config))
    })
    .clone()
    .map_err(Failure::tls)
}

/// What one negotiated session reports, read off the session itself: the protocol version,
/// the cipher suite, the protocol ALPN selected, and the certificate the peer presented.
fn negotiated_session(session: &::rustls::ClientConnection) -> Value {
    let protocol = session.protocol_version().map(protocol_name);
    let suite = session
        .negotiated_cipher_suite()
        .and_then(|suite| suite.suite().as_str());
    let certificate = session
        .peer_certificates()
        .and_then(<[::rustls::pki_types::CertificateDer<'_>]>::first)
        .map(|certificate| {
            ::base64::engine::general_purpose::STANDARD.encode(certificate.as_ref())
        });
    let alpn = session
        .alpn_protocol()
        .map(|protocol| String::from_utf8_lossy(protocol).to_string());
    json!({
        "protocol": protocol,
        "cipher": suite,
        "bits": suite.map(secret_bits),
        "alpn": alpn,
        "certificate": certificate,
    })
}

/// The name a TLS version is reported under, as `ssl.SSLSocket.version` names it.
fn protocol_name(version: ::rustls::ProtocolVersion) -> &'static str {
    match version {
        ::rustls::ProtocolVersion::TLSv1_3 => "TLSv1.3",
        ::rustls::ProtocolVersion::TLSv1_2 => "TLSv1.2",
        ::rustls::ProtocolVersion::TLSv1_1 => "TLSv1.1",
        ::rustls::ProtocolVersion::TLSv1_0 => "TLSv1",
        _ => "unknown",
    }
}

/// How many secret bits the negotiated suite's own name states: the key length of the BULK
/// cipher it names, which is what `ssl.SSLSocket.cipher` reports beside the suite. The digest
/// the name ends in is not that key, so the cipher's own part of the name is what is read.
fn secret_bits(suite: &str) -> u32 {
    if suite.contains("AES_256") || suite.contains("CHACHA20") {
        return 256;
    }
    if suite.contains("AES_128") {
        return 128;
    }
    0
}

/// The STREAM half of the `socket` module this runtime supplies, appended to the names and
/// the datagram sockets that module already states
/// ([`super::guest_http::runtime_stdlib_modules`]) — so there is ONE `socket` a handler
/// imports, and what this adds to it is the connections above.
pub const STREAM_SOURCE: &str = r#"

# ---------------------------------------------------------------------------
# The STREAM sockets of this runtime, and the connections the host holds for them.
#
# A connection is opened to an address this emulator serves a resource's own data plane at —
# the `host:port` the owning service published for a client to dial — and carries the bytes
# of whatever protocol that plane speaks on it. An address this emulator serves nothing at is
# answered the way a host answers one.

import base64 as _base64
import io as _io

# The numbers a host answers these failures with, which a program reads off `errno`.
_EBADF = 9
_EPIPE = 32
_ECONNRESET = 104
_EISCONN = 106
_ENOTCONN = 107
_ETIMEDOUT = 110
_ECONNREFUSED = 111
# `EAI_NONAME`: a name the resolver holds nothing for.
_EAI_NONAME = -2

# The ports a service NAME stands for, for a caller that passes one where a port goes. Only
# the two a client of a data plane names; any other name has no number here.
_SERVICE_PORTS = {"http": 80, "https": 443}


class _TlsFailed(OSError):
    """A TLS handshake or session failure, which the `ssl` module raises as its own error."""


def _stream_asked(verb, document):
    """One call on a connection, with a failure raised as the error a host answers."""
    answered = _json.loads(_host.net(verb, _json.dumps(document)))
    failed = answered.get("error")
    if failed:
        raise _stream_failure(failed)
    return answered


def _stream_failure(failed):
    """The exception one failure of this seam is, as a program catches it."""
    kind = failed.get("kind") or ""
    number = failed.get("errno")
    message = failed.get("message") or ""
    if kind == "name":
        return gaierror(number, message) if number is not None else gaierror(message)
    if kind == "timeout":
        return timeout(message)
    if kind == "refused":
        return ConnectionRefusedError(number, message)
    if kind == "reset":
        return ConnectionResetError(number, message)
    if kind == "tls":
        return _TlsFailed(message)
    if number is not None:
        return error(number, message)
    return error(message)


def _stream_port(port):
    """The port number a caller named, as a number or as a service name."""
    if port is None or port == "":
        return 0
    if isinstance(port, (bytes, bytearray)):
        port = bytes(port).decode("utf-8", "replace")
    if isinstance(port, str):
        if port.isdigit():
            return int(port)
        named = _SERVICE_PORTS.get(port.lower())
        if named is None:
            raise gaierror(_EAI_NONAME, "Servname not supported for ai_socktype")
        return named
    try:
        port = int(port)
    except (TypeError, ValueError):
        raise error("a connection is opened to a port") from None
    if not 0 <= port <= 65535:
        raise OverflowError("port must be 0-65535.")
    return port


def _stream_endpoint(address):
    """The `(host, port)` one address names, as a stream socket is connected to."""
    if not isinstance(address, (tuple, list)) or len(address) < 2:
        raise error("a stream socket is connected to a (host, port) address")
    host = address[0]
    if isinstance(host, (bytes, bytearray)):
        host = bytes(host).decode("utf-8", "replace")
    return str("" if host is None else host), _stream_port(address[1])


def _stream_seconds(value):
    """The timeout one call carries, in seconds, or `None` for a call that names none."""
    if value is None:
        return None
    try:
        seconds = float(value)
    except (TypeError, ValueError):
        return None
    return seconds if seconds > 0 else None


def _stream_handle(sock):
    """The connection one socket holds, or the error a socket holding none answers."""
    if sock._closed:
        raise error(_EBADF, "Bad file descriptor")
    if sock._handle is None:
        raise error(_ENOTCONN, "Transport endpoint is not connected")
    return sock._handle


def _stream_connect(sock, address):
    """Open the connection: the whole of what a stream socket's `connect` is."""
    host, port = _stream_endpoint(address)
    if sock._closed:
        raise error(_EBADF, "Bad file descriptor")
    if sock._handle is not None:
        raise error(_EISCONN, "Transport endpoint is already connected")
    answered = _stream_asked(
        "connect",
        {"host": host, "port": port, "timeout": _stream_seconds(sock._timeout)},
    )
    sock._handle = answered["handle"]
    sock._peer = (answered["peerHost"], answered["peerPort"])
    # The address the connection LEAVES from, as the host's own route lookup answered it: the
    # private address of the interface the function's VPC attachment holds, or this machine's
    # own interface for a plane served in this process. A destination there is no source for
    # leaves the socket bound to nothing.
    source = answered.get("sourceHost")
    if source:
        sock._source = (source, int(answered.get("sourcePort") or 0))
    return None


def _stream_bytes(data):
    if isinstance(data, (bytes, bytearray, memoryview)):
        return bytes(data)
    if isinstance(data, str):
        raise TypeError("a bytes-like object is required, not 'str'")
    return bytes(data)


def _stream_send(sock, data):
    """Write `data` on the connection, whole: the host carries it and reports what it wrote."""
    handle = _stream_handle(sock)
    payload = _stream_bytes(data)
    if not payload:
        return 0
    answered = _stream_asked(
        "send",
        {
            "handle": handle,
            "data": _base64.b64encode(payload).decode("ascii"),
            "timeout": _stream_seconds(sock._timeout),
        },
    )
    return int(answered.get("sent") or 0)


def _stream_recv(sock, size):
    """Read what has arrived, up to `size` bytes. An empty answer is the peer's end of file."""
    handle = _stream_handle(sock)
    try:
        size = int(size)
    except (TypeError, ValueError):
        raise error("the number of bytes to read is a number") from None
    if size < 0:
        raise ValueError("negative buffersize in recv")
    if size == 0:
        return b""
    answered = _stream_asked(
        "recv",
        {"handle": handle, "size": size, "timeout": _stream_seconds(sock._timeout)},
    )
    return _base64.b64decode(answered.get("data") or "")


def _stream_recv_into(sock, buffer, nbytes=None):
    view = memoryview(buffer)
    size = len(view) if not nbytes else min(int(nbytes), len(view))
    data = _stream_recv(sock, size)
    view[: len(data)] = data
    return len(data)


def _stream_close(sock):
    """Close the connection. A socket that holds none closes nothing and raises nothing."""
    handle = sock._handle
    sock._handle = None
    sock._peer = None
    sock._source = None
    if handle is None:
        return None
    try:
        _stream_asked("close", {"handle": handle})
    except OSError:
        return None
    return None


def _stream_shutdown(sock, how):
    _stream_asked("shutdown", {"handle": _stream_handle(sock), "how": int(how)})
    return None


def _stream_tls(sock, server_hostname):
    """Negotiate a TLS session on the connection, and answer what it negotiated."""
    return _stream_asked(
        "tls",
        {
            "handle": _stream_handle(sock),
            "timeout": _stream_seconds(sock._timeout),
        },
    )


def _stream_untls(sock):
    """Close the TLS session and leave the connection in the clear."""
    return _stream_asked("untls", {"handle": _stream_handle(sock)})


class _SocketIO(_io.RawIOBase):
    """The raw reading and writing of one socket, which `makefile`'s buffers are built on."""

    def __init__(self, sock, mode):
        _io.RawIOBase.__init__(self)
        self._socket = sock
        self._stream_mode = mode

    def readinto(self, buffer):
        view = memoryview(buffer)
        data = self._socket.recv(len(view))
        view[: len(data)] = data
        return len(data)

    def write(self, data):
        return self._socket.send(bytes(data))

    def readable(self):
        return "r" in self._stream_mode

    def writable(self):
        return "w" in self._stream_mode

    def seekable(self):
        return False

    def fileno(self):
        return self._socket.fileno()

    @property
    def mode(self):
        return self._stream_mode + "b"


def _stream_makefile(sock, mode="r", buffering=None, encoding=None, errors=None, newline=None):
    """A file over the connection, as the standard library's own `makefile` builds one."""
    _stream_handle(sock)
    if set(mode) - set("rwb"):
        raise ValueError("invalid mode %r (only r, w, b allowed)" % (mode,))
    writing = "w" in mode
    reading = "r" in mode or not writing
    binary = "b" in mode
    raw = _SocketIO(sock, ("r" if reading else "") + ("w" if writing else ""))
    size = _io.DEFAULT_BUFFER_SIZE if buffering is None or buffering < 0 else int(buffering)
    if size == 0:
        if not binary:
            raise ValueError("unbuffered streams must be binary")
        return raw
    if reading and writing:
        buffered = _io.BufferedRWPair(raw, raw, size)
    elif reading:
        buffered = _io.BufferedReader(raw, size)
    else:
        buffered = _io.BufferedWriter(raw, size)
    if binary:
        return buffered
    return _io.TextIOWrapper(buffered, encoding, errors, newline)
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an address NO plane of this emulator answers at is answered the way a host
    /// answers one — a refused connection for an address of this machine, no route for an
    /// address of another host, and a name that resolves to nothing for a name this
    /// environment's resolver holds no address under.
    ///
    /// The rows span what the reading generalises over: an address of this machine, its
    /// loopback name, a routable address, a host name of AWS's own naming — the shape a
    /// control plane publishes for a plane this emulator has opened nothing for — and a name
    /// the resolver DOES hold an address for, which has a route rather than no name.
    #[test]
    fn an_address_no_plane_answers_at_is_answered_as_a_host_answers_one() {
        let rows: &[(&str, &str, &str, Option<i32>)] = &[
            ("an address of this machine", "127.0.0.1", "refused", Some(ECONNREFUSED)),
            ("its loopback name", "localhost", "refused", Some(ECONNREFUSED)),
            ("an address of another host", "10.0.7.9", "other", Some(ENETUNREACH)),
            (
                "a host of AWS's own naming",
                "cluster-x.cluster-abc.us-east-1.docdb.amazonaws.com",
                "name",
                Some(EAI_NONAME),
            ),
            ("a name that resolves", "example.com", "other", Some(ENETUNREACH)),
        ];
        for (case, host, kind, errno) in rows {
            let failed = nothing_answers(None, host);
            assert_eq!(
                failed.kind, *kind,
                "case {case:?}: expected the {kind} failure, got {:?}",
                failed.kind
            );
            assert_eq!(
                failed.errno, *errno,
                "case {case:?}: expected errno {errno:?}, got {:?}",
                failed.errno
            );
        }
    }

    /// RULE: no socket call waits longer than the run has left, than the guest asked for, or
    /// than the ceiling a call with no timeout of its own gets — and never for no time at
    /// all, because a run with nothing left is answered by its own deadline.
    ///
    /// The rows differ along which of the three bounds is the smallest, and along the values
    /// a guest can hand over for one (none, zero, a negative, a magnitude no span can hold,
    /// something that is not a number at all).
    #[test]
    fn every_wait_is_clamped_to_what_the_run_has_left() {
        let run = Duration::from_secs(5);
        let rows: &[(&str, Value, Duration, Duration)] = &[
            ("what the guest asked for", json!({"timeout": 2.0}), run, Duration::from_secs(2)),
            ("what the run has left", json!({"timeout": 90.0}), run, run),
            ("the ceiling, with no timeout named", json!({}), Duration::from_secs(600), WAIT_CEILING),
            ("a zero timeout", json!({"timeout": 0.0}), run, run),
            ("a negative timeout", json!({"timeout": -3.0}), run, run),
            ("a timeout that is no number", json!({"timeout": "soon"}), run, run),
            // A magnitude no span can hold is the ceiling, like any other value past it: the
            // guest's float is clamped BEFORE it is made a span, so nothing of it can fault
            // the conversion.
            ("a timeout no span can hold", json!({"timeout": 1e30}), Duration::from_secs(600), WAIT_CEILING),
            ("a timeout past every bound", json!({"timeout": 1e300}), run, run),
            ("a run with nothing left", json!({"timeout": 10.0}), Duration::ZERO, Duration::from_millis(1)),
        ];
        for (case, asked, within, expected) in rows {
            let waiting = clamped(asked, *within);
            assert_eq!(
                waiting, *expected,
                "case {case:?}: expected a wait of {expected:?}, got {waiting:?}"
            );
        }
    }

    /// RULE: how many secret bits a session has is read off the SUITE it negotiated, not
    /// chosen — the key length the suite's own name states.
    #[test]
    fn the_secret_bits_are_read_off_the_negotiated_suite() {
        let rows: &[(&str, u32)] = &[
            ("TLS13_AES_128_GCM_SHA256", 128),
            ("TLS13_AES_256_GCM_SHA384", 256),
            ("TLS13_CHACHA20_POLY1305_SHA256", 256),
            ("TLS_NULL_WITH_NULL_NULL", 0),
        ];
        for (suite, expected) in rows {
            let bits = secret_bits(suite);
            assert_eq!(
                bits, *expected,
                "case {suite:?}: expected {expected} secret bits, got {bits}"
            );
        }
    }

    /// A call that is not one this seam makes, and a document that is not a document, are
    /// reported as calls that were not made rather than taking the run down.
    #[tokio::test]
    async fn a_call_this_seam_does_not_make_is_reported_as_one_that_was_not_made() {
        let sockets = Sockets::default();
        let runtime = Handle::current();
        for (case, verb, document) in [
            ("a verb this seam has no call for", "listen", "{}"),
            ("a document that is not one", "connect", "not a document"),
            ("a handle that names no connection", "recv", "{\"handle\":7,\"size\":1}"),
        ] {
            let answered =
                sockets.answered(None, None, &runtime, verb, document, Duration::from_secs(1));
            assert!(
                answered.contains("\"error\""),
                "case {case:?}: expected a reported failure, got {answered}"
            );
        }
        assert_eq!(
            sockets.open(),
            0,
            "expected no connection to have been opened at all"
        );
    }

    /// One row of the table below: the case, the destination, the source the ROUTE answers,
    /// the errno that route is refused with, and the errno a DATAGRAM to it is refused with.
    type RouteRow = (&'static str, &'static str, Option<&'static str>, Option<i64>, i64);

    /// RULE: the route a datagram takes is answered off the function's VPC attachment: this
    /// machine's own interface is where the planes served in this process are, and a function
    /// attached to NO VPC has no route to any other address. A datagram handed over for an
    /// address of this machine is REFUSED — the one datagram service here is the X-Ray daemon,
    /// which a handler delivers to without reaching this seam — and one with no route is never
    /// handed over at all.
    ///
    /// The rows differ along where the destination sits: this machine, its loopback name, an
    /// address of another host, and a name nothing resolves.
    #[tokio::test]
    async fn the_route_of_a_function_attached_to_no_vpc_reaches_this_machine_alone() {
        let sockets = Sockets::default();
        let runtime = Handle::current();
        // Per row: the source the ROUTE answers, the errno that route is refused with, and
        // the errno a DATAGRAM to it is refused with.
        let rows: &[RouteRow] = &[
            (
                "this machine",
                "127.0.0.1",
                Some("127.0.0.1"),
                None,
                i64::from(ECONNREFUSED),
            ),
            (
                "its loopback name",
                "localhost",
                Some("127.0.0.1"),
                None,
                i64::from(ECONNREFUSED),
            ),
            (
                "an address of another host",
                "10.0.7.9",
                None,
                Some(i64::from(ENETUNREACH)),
                i64::from(ENETUNREACH),
            ),
            (
                "a name nothing resolves",
                "broker.example.com",
                None,
                Some(i64::from(EAI_NONAME)),
                i64::from(EAI_NONAME),
            ),
            // A name this environment's resolver DOES hold an address for is routed toward
            // that address, which a function attached to no VPC has no route to.
            (
                "a name that resolves to an address of another host",
                "example.com",
                None,
                Some(i64::from(ENETUNREACH)),
                i64::from(ENETUNREACH),
            ),
        ];
        for (case, host, expected, route_errno, datagram_errno) in rows {
            let asked = json!({ "host": host, "port": 53, "data": "" }).to_string();
            let routed = sockets.answered(
                None,
                None,
                &runtime,
                "route",
                &asked,
                Duration::from_secs(1),
            );
            let reported: Value = ::serde_json::from_str(&routed).expect("an answer document");
            assert_eq!(
                reported.get("source").and_then(Value::as_str),
                *expected,
                "case {case:?} (route): expected the source {expected:?}, got {routed}"
            );
            assert_eq!(
                reported
                    .get("error")
                    .and_then(|error| error.get("errno"))
                    .and_then(Value::as_i64),
                *route_errno,
                "case {case:?} (route): expected errno {route_errno:?}, got {routed}"
            );
            if expected.is_some() {
                // The port is one lent out of the ephemeral range, as a host lends one to a
                // socket that bound none itself.
                let port = reported
                    .get("sourcePort")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                assert!(
                    (u64::from(EPHEMERAL_PORT_FIRST)..=u64::from(EPHEMERAL_PORT_LAST))
                        .contains(&port),
                    "case {case:?} (route): expected a lent ephemeral port, got {routed}"
                );
            }
            // No datagram of a function attached to no VPC is delivered anywhere: nothing of
            // this emulator answers one at this machine, and every other address has no
            // route.
            let delivered = sockets.answered(
                None,
                None,
                &runtime,
                "datagram",
                &asked,
                Duration::from_secs(1),
            );
            let reported: Value = ::serde_json::from_str(&delivered).expect("an answer document");
            assert_eq!(
                reported
                    .get("error")
                    .and_then(|error| error.get("errno"))
                    .and_then(Value::as_i64),
                Some(*datagram_errno),
                "case {case:?} (datagram): expected errno {datagram_errno}, got {delivered}"
            );
            assert_eq!(
                reported.get("data").unwrap_or(&Value::Null),
                &Value::Null,
                "case {case:?} (datagram): expected no reply to come back, got {delivered}"
            );
        }
    }

    /// RULE: a connection is opened only to a plane a service RECORDED, and an address no
    /// plane of this emulator answers at is answered the way a host answers one — with no
    /// connection opened and nothing dialled.
    ///
    /// The rows span what the reading generalises over: an address of this machine nothing was
    /// recorded at, an address of another host, and a host of AWS's own naming — the shape a
    /// control plane publishes for a plane this emulator has opened nothing for.
    #[tokio::test]
    async fn an_address_no_plane_answers_at_opens_no_connection() {
        let engine = ::engine::InMemoryEngine::new();
        let planes = DialablePlanes::of(&engine).expect("the engine vends the records");
        let sockets = Sockets::default();
        let runtime = Handle::current();
        // Addresses from the blocks RFC 5737 reserves for documentation, so neither names a
        // host of anything.
        let rows: &[(&str, &str, u16, i64)] = &[
            ("an address of this machine", "127.0.0.1", 27017, i64::from(ECONNREFUSED)),
            ("an address of another host", "203.0.113.9", 27017, i64::from(ENETUNREACH)),
            (
                "a host of AWS's own naming",
                "cluster-x.cluster-abc.us-east-1.docdb.amazonaws.com",
                27017,
                i64::from(EAI_NONAME),
            ),
            (
                "a name that resolves to an address nothing is served at",
                "example.com",
                443,
                i64::from(ENETUNREACH),
            ),
        ];
        for (case, host, port, errno) in rows {
            let asked = json!({ "host": host, "port": port, "timeout": 1.0 }).to_string();
            let answered = sockets.answered(
                Some(&planes),
                None,
                &runtime,
                "connect",
                &asked,
                Duration::from_secs(1),
            );
            let reported: Value = ::serde_json::from_str(&answered).expect("an answer document");
            assert!(
                reported.get("handle").is_none(),
                "case {case:?}: expected no connection to be opened, got {answered}"
            );
            let reported = reported
                .get("error")
                .and_then(|error| error.get("errno"))
                .and_then(Value::as_i64);
            assert_eq!(
                reported,
                Some(*errno),
                "case {case:?}: expected errno {errno}, got {answered}"
            );
        }
        assert_eq!(
            sockets.open(),
            0,
            "expected no connection to have been opened at all"
        );
    }

    /// RULE: a connection this seam opens STATES the address it leaves from, so the plane
    /// serving it can name the client it is serving — the private address of the function's own
    /// interface in the VPC it is attached to, and, for a function attached to NO VPC, the
    /// interface of this machine the route lookup answered with on the port the socket was
    /// lent. Never nothing, so no plane that states its client has to invent one.
    ///
    /// This is the row the seam resolves with no inventory to read: a function attached to no
    /// VPC. The in-VPC row is asserted end to end, against the address Amazon EC2 holds for the
    /// interface Lambda stood up, by
    /// `core/router/tests/all/elasticache_slow_log_client_for_every_attachment.rs`.
    #[tokio::test]
    async fn a_connection_states_the_address_it_leaves_from() {
        /// A plane that answers every connection with the address its client stated, which is
        /// what a protocol that names its client reads off the connection.
        struct Stating;

        impl ::emulator_server::NativeHandler for Stating {
            fn serve(
                &self,
                conn: ListenerConn,
            ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = ()> + Send>> {
                let stated = ::service_common::dialable_data_planes::connected_from()
                    .map_or_else(|| "nothing was stated".to_string(), |from| from.to_string());
                Box::pin(async move {
                    let mut conn = conn;
                    let _written = conn.write_all(stated.as_bytes()).await;
                    let _flushed = conn.flush().await;
                })
            }
        }

        let engine = ::engine::InMemoryEngine::new();
        let planes = DialablePlanes::of(&engine).expect("the engine vends the records");
        let port = 37_017_u16;
        let plane = ::service_common::dialable_data_planes::served_in_process(
            ::std::sync::Arc::new(Stating),
            None,
        );
        let _open = planes.opening([format!("127.0.0.1:{port}")], "elasticache", &plane);
        let sockets = Sockets::default();
        let runtime = Handle::current();
        // The seam is the host half of a BLOCKING guest call — it drives the plane's own task
        // on the runtime while the guest's thread waits — so it is called the way a running
        // handler calls it: off the runtime's own threads.
        let (opened, received) = ::tokio::task::spawn_blocking(move || {
            let opened = sockets.answered(
                Some(&planes),
                None,
                &runtime,
                "connect",
                &json!({ "host": "127.0.0.1", "port": port, "timeout": 5.0 }).to_string(),
                Duration::from_secs(5),
            );
            let handle = ::serde_json::from_str::<Value>(&opened)
                .ok()
                .and_then(|answer| answer.get("handle").and_then(Value::as_u64))
                .unwrap_or_default();
            let received = sockets.answered(
                Some(&planes),
                None,
                &runtime,
                "recv",
                &json!({ "handle": handle, "size": 64, "timeout": 5.0 }).to_string(),
                Duration::from_secs(5),
            );
            (opened, received)
        })
        .await
        .expect("the guest's calls ran");
        let opened: Value = ::serde_json::from_str(&opened).expect("an answer document");
        assert!(
            opened.get("handle").and_then(Value::as_u64).is_some(),
            "the connection must be opened, got {opened}"
        );
        let received: Value = ::serde_json::from_str(&received).expect("an answer document");
        let carried = received
            .get("data")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let read = ::base64::engine::general_purpose::STANDARD
            .decode(carried)
            .unwrap_or_else(|error| panic!("the bytes read back ({error}): {received}"));
        let stated = String::from_utf8(read)
            .unwrap_or_else(|error| panic!("the address the plane was told ({error})"));
        let addressed: ::std::net::SocketAddr = stated.parse().unwrap_or_else(|error| {
            panic!("the plane must be told an address:port, got {stated:?} ({error})")
        });
        assert!(
            addressed.ip().is_loopback(),
            "a connection to a plane served in this process leaves from this machine's own \
             interface, got {stated:?}"
        );
        assert!(
            (EPHEMERAL_PORT_FIRST..=EPHEMERAL_PORT_LAST).contains(&addressed.port()),
            "the connection must state the port it was lent, got {stated:?}"
        );
        let source = addressed.ip().to_string();
        assert_eq!(
            opened.get("sourceHost").and_then(Value::as_str),
            Some(source.as_str()),
            "the guest's own reading of the socket names the same interface, got {opened}"
        );
    }
}
