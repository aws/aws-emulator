// SPDX-License-Identifier: CC0-1.0

//! The standard-library modules whose whole CAPABILITY this runtime has none of: `http2`,
//! `dgram`, `cluster`, `inspector` (and its promise form), `repl`, `wasi` and `trace_events`.
//!
//! A managed `nodejs*` runtime is a promise about an interpreter AND about the standard
//! library that comes with it, so a handler — or a bundle it ships — whose module body says
//! `require('http2')` loads on the runtime it was written for, and so it has to here. A
//! module of that standard library this emulator registers no capability for is SUPPLIED on
//! the resolver path instead of being absent from it: what is refused is the CALL that needs
//! the capability, never the import, because an import that fails points the developer at a
//! deployment package that is not at fault. The same decision stands behind this crate's
//! `child_process` ([`super::child_process`]) and its socket modules ([`super::sockets`]),
//! and the reason is the same one: the JavaScript engine an invocation runs in reaches the
//! host through the host functions the runtime installs and through nothing else
//! ([`super::super`] module docs), and none of them is a datagram socket, an HTTP/2 session,
//! a process to fork, an inspector, a terminal to read a REPL from or a WebAssembly system
//! interface.
//!
//! So each module here states the NAMES node states, performs the pieces that are
//! COMPUTATION — the settings node documents as its defaults, which of a cluster's roles this
//! process is, the categories a tracing object was asked for — and reports every call that
//! would need the absent capability as the failure a host answers for a system call that is
//! not there (`ENOSYS`), thrown where node throws and emitted on the object where node
//! reports it there. Nothing here invents a session, a socket, a worker, an inspector message
//! or a WebAssembly instance for work nothing ran.
//!
//! What a handler CAN reach of the network is the one request this runtime sends for it,
//! which is the `http`/`https` client ([`super::http`]) and the HTTP server of this run those
//! two modules serve; what it can reach of concurrency is `worker_threads`
//! ([`super::worker_threads`]).

/// One module of this group: the head every one of them opens with — the number a system call
/// that is not there answers with, the failure built from it, the deferred report of one on an
/// emitter, and the prototype chaining each class uses — and then that module's own body.
///
/// The head is written ONCE here and concatenated at COMPILE time, so each module is one
/// `&'static str` the resolver hands the engine exactly as every other core module's source
/// is.
macro_rules! absent_capability_module {
    ($(#[$doc:meta])* $name:ident = $body:literal) => {
        $(#[$doc])*
        pub(super) const $name: &str = concat!(
            r##"
'use strict';

var EventEmitter = require('events');

// The number a system call that is not there answers with. node carries it negated in
// `errno`, as the result it came from.
var ENOSYS = 38;

// The failure of a call this runtime has no capability for, as node builds one for a system
// call that is not there.
function absent(syscall) {
    var error = new Error(syscall + ' ENOSYS');
    error.errno = -ENOSYS;
    error.code = 'ENOSYS';
    error.syscall = syscall;
    return error;
}

// Report `error` on `emitter` once the caller has had the chance to attach its handlers,
// which is how node reports the failure of an asynchronous entry point: to the callback the
// call was given, and otherwise as the object's own `error` event.
function reported(emitter, error, callback) {
    Promise.resolve().then(function () {
        if (typeof callback === 'function') {
            callback(error);
        }
        if (emitter.listenerCount('error') > 0) {
            emitter.emit('error', error);
        } else if (typeof callback !== 'function') {
            Promise.reject(error);
        }
    });
    return emitter;
}

function inherit(child, parent) {
    child.prototype = Object.create(parent.prototype);
    child.prototype.constructor = child;
}
"##,
            $body
        );
    };
}

absent_capability_module! {
    /// `http2`: the HTTP/2 client and server, on a runtime that holds no session.
    HTTP2_SOURCE = r##"
// The pseudo-headers and the header and method names a program reads off this module, each
// the name HTTP/2 itself fixes.
var constants = {
    HTTP2_HEADER_AUTHORITY: ':authority',
    HTTP2_HEADER_METHOD: ':method',
    HTTP2_HEADER_PATH: ':path',
    HTTP2_HEADER_PROTOCOL: ':protocol',
    HTTP2_HEADER_SCHEME: ':scheme',
    HTTP2_HEADER_STATUS: ':status',
    HTTP2_HEADER_CONTENT_LENGTH: 'content-length',
    HTTP2_HEADER_CONTENT_TYPE: 'content-type',
    HTTP2_METHOD_DELETE: 'DELETE',
    HTTP2_METHOD_GET: 'GET',
    HTTP2_METHOD_HEAD: 'HEAD',
    HTTP2_METHOD_PATCH: 'PATCH',
    HTTP2_METHOD_POST: 'POST',
    HTTP2_METHOD_PUT: 'PUT'
};

// "The default settings for an Http2Session instance": the values node documents, which are
// computation over nothing the host holds.
function getDefaultSettings() {
    return {
        headerTableSize: 4096,
        enablePush: true,
        initialWindowSize: 65535,
        maxFrameSize: 16384,
        maxConcurrentStreams: 4294967295,
        maxHeaderListSize: 65535,
        enableConnectProtocol: false
    };
}

// A session is held over a socket, which this runtime has none of: `connect` hands back the
// object a program attaches its handlers to and the failure is reported on it.
function Http2Session() {
    EventEmitter.call(this);
    this.closed = false;
    this.destroyed = true;
    this.state = {};
}

inherit(Http2Session, EventEmitter);

Http2Session.prototype.request = function () { throw absent('http2_request'); };
Http2Session.prototype.settings = function () { throw absent('http2_settings'); };
Http2Session.prototype.ping = function () { return false; };
Http2Session.prototype.destroy = function () { this.destroyed = true; };
Http2Session.prototype.ref = function () { return this; };
Http2Session.prototype.unref = function () { return this; };

Http2Session.prototype.close = function (callback) {
    this.closed = true;
    if (typeof callback === 'function') {
        Promise.resolve().then(callback);
    }
};

function Http2Server() {
    EventEmitter.call(this);
    this.listening = false;
}

inherit(Http2Server, EventEmitter);

Http2Server.prototype.listen = function () {
    return reported(this, absent('http2_listen'), undefined);
};

Http2Server.prototype.address = function () { return null; };

Http2Server.prototype.close = function (callback) {
    if (typeof callback === 'function') {
        Promise.resolve().then(callback);
    }
    return this;
};

function connect(authority, options, listener) {
    var session = new Http2Session();
    session.authority = String(authority);
    return reported(
        session,
        absent('http2_connect'),
        typeof options === 'function' ? options : listener
    );
}

function createServer() {
    return new Http2Server();
}

module.exports = {
    constants: constants,
    getDefaultSettings: getDefaultSettings,
    getPackedSettings: function () { throw absent('http2_pack_settings'); },
    getUnpackedSettings: function () { throw absent('http2_unpack_settings'); },
    connect: connect,
    createServer: createServer,
    createSecureServer: createServer,
    Http2Session: Http2Session,
    Http2Server: Http2Server,
    sensitiveHeaders: Symbol('nodejs.http2.sensitiveHeaders')
};
"##
}

absent_capability_module! {
    /// `dgram`: the datagram socket, on a runtime that holds none.
    DGRAM_SOURCE = r##"
// A datagram socket is a host socket, which an invocation of this emulator has none of: the
// object a program binds and sends through is here, and each call that would need the socket
// itself reports the failure a host answers for a system call that is not there.
function Socket(options) {
    EventEmitter.call(this);
    this.type = typeof options === 'string'
        ? options
        : (options && options.type ? String(options.type) : 'udp4');
}

inherit(Socket, EventEmitter);

function trailingCallback(args) {
    var last = args.length === 0 ? undefined : args[args.length - 1];
    return typeof last === 'function' ? last : undefined;
}

Socket.prototype.bind = function () {
    return reported(this, absent('bind'), trailingCallback(arguments));
};

Socket.prototype.connect = function () {
    return reported(this, absent('connect'), trailingCallback(arguments));
};

Socket.prototype.send = function () {
    var callback = trailingCallback(arguments);
    var error = absent('send');
    if (callback !== undefined) {
        Promise.resolve().then(function () { callback(error); });
        return undefined;
    }
    reported(this, error, undefined);
    return undefined;
};

Socket.prototype.close = function (callback) {
    if (typeof callback === 'function') {
        Promise.resolve().then(callback);
    }
    return this;
};

Socket.prototype.address = function () { throw absent('getsockname'); };
Socket.prototype.remoteAddress = function () { throw absent('getpeername'); };
Socket.prototype.addMembership = function () { throw absent('setsockopt'); };
Socket.prototype.dropMembership = function () { throw absent('setsockopt'); };
Socket.prototype.setBroadcast = function () { throw absent('setsockopt'); };
Socket.prototype.setTTL = function () { throw absent('setsockopt'); };
Socket.prototype.setMulticastTTL = function () { throw absent('setsockopt'); };
Socket.prototype.setRecvBufferSize = function () { throw absent('setsockopt'); };
Socket.prototype.setSendBufferSize = function () { throw absent('setsockopt'); };
Socket.prototype.getRecvBufferSize = function () { throw absent('getsockopt'); };
Socket.prototype.getSendBufferSize = function () { throw absent('getsockopt'); };
Socket.prototype.ref = function () { return this; };
Socket.prototype.unref = function () { return this; };

function createSocket(options, listener) {
    var socket = new Socket(options);
    var message = typeof options === 'function' ? options : listener;
    if (typeof message === 'function') {
        socket.on('message', message);
    }
    return socket;
}

module.exports = {
    createSocket: createSocket,
    Socket: Socket,
};
"##
}

absent_capability_module! {
    /// `cluster`: the worker processes, on a runtime that forks none.
    CLUSTER_SOURCE = r##"
// Which role this process is IS computation over what the run holds: an invocation runs in
// one process and that process is the primary — "the primary process ... can fork worker
// processes" — so nothing here is a worker.
function Cluster() {
    EventEmitter.call(this);
    this.isPrimary = true;
    this.isMaster = true;
    this.isWorker = false;
    this.worker = undefined;
    this.workers = {};
    this.settings = {};
    this.SCHED_NONE = 1;
    this.SCHED_RR = 2;
    this.schedulingPolicy = this.SCHED_RR;
}

inherit(Cluster, EventEmitter);

// "Spawn a new worker process": a worker is a process of its own, which this runtime starts
// none of, so the call reports the failure a host answers for a fork that did not happen.
Cluster.prototype.fork = function () {
    throw absent('fork');
};

// "After calling setupPrimary ... the settings are frozen": the settings a call states are
// held, because holding them needs nothing of the host.
Cluster.prototype.setupPrimary = function (settings) {
    if (settings !== null && typeof settings === 'object') {
        var names = Object.keys(settings);
        for (var at = 0; at < names.length; at += 1) {
            this.settings[names[at]] = settings[names[at]];
        }
    }
    this.emit('setup', this.settings);
};

Cluster.prototype.setupMaster = Cluster.prototype.setupPrimary;

Cluster.prototype.disconnect = function (callback) {
    if (typeof callback === 'function') {
        Promise.resolve().then(callback);
    }
};

module.exports = new Cluster();
"##
}

absent_capability_module! {
    /// `inspector`: the debugging protocol, on a runtime with no inspector attached.
    INSPECTOR_SOURCE = r##"
// A session speaks to the inspector of the process, which an invocation of this emulator has
// none of: the names are here and every call that would need it is refused.
function Session() {
    EventEmitter.call(this);
}

inherit(Session, EventEmitter);

Session.prototype.connect = function () { throw absent('inspector_connect'); };
Session.prototype.connectToMainThread = function () { throw absent('inspector_connect'); };
Session.prototype.disconnect = function () { return undefined; };

Session.prototype.post = function (method, params, callback) {
    var answer = typeof params === 'function' ? params : callback;
    var error = absent('inspector_post');
    if (typeof answer === 'function') {
        Promise.resolve().then(function () { answer(error); });
        return undefined;
    }
    throw error;
};

module.exports = {
    Session: Session,
    // "Activate inspector on host:port": there is no inspector to activate.
    open: function () { throw absent('inspector_open'); },
    close: function () { return undefined; },
    // "Return the URL of the active inspector, or undefined if there is none."
    url: function () { return undefined; },
    waitForDebugger: function () { throw absent('inspector_wait'); },
    console: globalThis.console,
};
"##
}

absent_capability_module! {
    /// `inspector/promises`: the same module, whose `post` answers a promise.
    INSPECTOR_PROMISES_SOURCE = r##"
var inspector = require('inspector');

// "The same as the inspector module, but the post method returns a promise."
function Session() {
    inspector.Session.call(this);
}

inherit(Session, inspector.Session);

Session.prototype.post = function () {
    return Promise.reject(absent('inspector_post'));
};

module.exports = {
    Session: Session,
    open: inspector.open,
    close: inspector.close,
    url: inspector.url,
    waitForDebugger: inspector.waitForDebugger,
    console: inspector.console,
};
"##
}

absent_capability_module! {
    /// `repl`: the read-eval-print loop, on a runtime with no terminal to read one from.
    REPL_SOURCE = r##"
// A REPL reads from the input stream of a terminal, which an execution environment has none
// of ([`super::environment_views`]'s `tty`), so starting one is refused rather than answered
// with a server nothing speaks to. What is computation is here: `writer` is the inspection
// a REPL prints a value with.
function REPLServer() {
    throw absent('repl_start');
}

function Recoverable(error) {
    this.err = error;
}

module.exports = {
    start: function () { throw absent('repl_start'); },
    writer: function (value) { return require('util').inspect(value); },
    REPLServer: REPLServer,
    Recoverable: Recoverable,
    REPL_MODE_SLOPPY: Symbol('repl-sloppy'),
    REPL_MODE_STRICT: Symbol('repl-strict'),
};
"##
}

absent_capability_module! {
    /// `wasi`: the WebAssembly system interface, on a runtime that grants none.
    WASI_SOURCE = r##"
// "The WASI API provides an implementation of the WebAssembly System Interface
// specification": that interface GRANTS a WebAssembly instance the host's files, clock and
// arguments, and an invocation of this emulator grants none — its engine reaches the host
// only through the functions the runtime installs. So the class is here and constructing one
// is refused, rather than answered with an instance whose system calls nothing serves.
function WASI() {
    throw absent('wasi_init');
}

module.exports = { WASI: WASI };
"##
}

absent_capability_module! {
    /// `trace_events`: the tracing categories, on a runtime that writes no trace file.
    TRACE_EVENTS_SOURCE = r##"
// A tracing object names categories and is enabled or disabled, which is computation over
// what the call itself states; what it does NOT do here is write a trace file, because an
// invocation reaches no host file. Nothing is reported as RECORDED, so no trace is invented:
// what is answered is which categories are enabled.
function Tracing(categories) {
    var held = Array.isArray(categories) ? categories.slice().map(String) : [];
    this.enabled = false;
    this.categories = held.join(',');
}

Tracing.prototype.enable = function () { this.enabled = true; };
Tracing.prototype.disable = function () { this.enabled = false; };

var created = [];

function createTracing(options) {
    if (options === null || typeof options !== 'object' || !Array.isArray(options.categories)) {
        throw new TypeError('The "options.categories" property must be an instance of Array.');
    }
    var tracing = new Tracing(options.categories);
    created.push(tracing);
    return tracing;
}

// "Returns a comma-separated list of all currently-enabled trace event categories."
function getEnabledCategories() {
    var names = [];
    for (var at = 0; at < created.length; at += 1) {
        if (created[at].enabled && created[at].categories !== '') {
            names.push(created[at].categories);
        }
    }
    return names.join(',');
}

module.exports = {
    createTracing: createTracing,
    getEnabledCategories: getEnabledCategories,
};
"##
}
