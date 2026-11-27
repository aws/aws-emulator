// SPDX-License-Identifier: CC0-1.0

//! The node CORE MODULES a handler run in this emulator can import, each as the JavaScript
//! source of a CommonJS module.
//!
//! A deployment package's own files are the caller's; these are the ones a Node program
//! expects of its runtime rather than of its package. They are loaded by the very
//! mechanism a package file is ([`super::runtime`]'s `require`), so a core module is
//! compiled, cached and reachable under both its bare and its `node:` name exactly as a
//! package module is.
//!
//! Almost every one of them is COMPUTATION over values the run already holds: text, paths,
//! formatting, compression, event dispatch — and the ones that are the SAME for every
//! service running caller-supplied Node.js code are compiled from the source this emulator
//! keeps once ([`::service_common::node_guest_modules`], reached through [`pure_modules`]
//! and [`system`]'s text codecs) rather than from a copy of their own. The exceptions are
//! the two that answer from this
//! EMULATOR rather than from the run — the HTTP client, whose one request the host carries
//! into this emulator and no further ([`http`]), and the name service, which is the one
//! resolver of the execution environment ([`dns`]) — and neither reaches a host file, socket,
//! process or environment either, because there is no host function here to reach one with:
//! the whole host surface of a run is what [`super::install_host`] installs. A module a Node
//! program expects and this emulator does not supply is simply not resolvable, so a handler
//! that needs one fails to import exactly as it would if the dependency were missing from its
//! package; nothing here pretends to be a module it is not.
//!
//! A module whose whole capability this runtime HAS NONE OF is supplied all the same, with
//! the calls that need that capability refused ([`child_process`], [`sockets`],
//! [`host_absent`]): the standard library is part of what a managed runtime promises, so an
//! import of one of its modules has to load — a program whose module body imports one and
//! whose running code handles the refusal is a program that runs — and what is answered for
//! is the CALL rather than the import. The same promise covers the modules whose whole
//! content is a VIEW of what the run already holds ([`environment_views`]) and the CONSOLE
//! that library installs ([`console`]), whose whole method surface is here because a program
//! calls those methods on the global and on a console it builds itself alike.

/// The byte container and its codecs.
mod buffer;
/// The child-process interface, on a runtime that starts no child.
mod child_process;
/// The records a program writes, and the console it builds its own over its own streams.
pub(super) mod console;
/// The digests, keyed digests and random bytes.
mod crypto;
/// The three modules a program observes its own execution through.
mod diagnostics;
/// The name service of the execution environment.
mod dns;
/// The modules whose whole content is a view of something this environment already holds.
mod environment_views;
/// The in-memory filesystem a function has.
mod fs;
/// The modules whose whole capability this runtime has none of.
mod host_absent;
/// The HTTP client a handler sends a request of its own with.
mod http;
/// The lines a program reads off a stream, and the source it compiles at runtime.
mod scripting;
/// The stream-socket interfaces, on a runtime that holds no socket.
mod sockets;
/// The stream object model and the text decoder over it.
mod streams;
/// The environment report and the assertions, beside the two text codecs this emulator
/// keeps once for every service that runs Node.js code.
mod system;
/// The scheduling calls, as modules.
mod timers;
/// The workers a program hands part of its work to.
mod worker_threads;
/// The compression a program asks its runtime for.
mod zlib;
/// The path algebra, the value formatter and the event emitter, from the source this
/// emulator keeps once for every service that runs Node.js code.
mod pure_modules;
pub use pure_modules::*;

/// Every core module, by the name a handler imports it under.
pub(super) const CORE_MODULES: &[(&str, &str)] = &[
    ("path", PATH_SOURCE),
    ("util", UTIL_SOURCE),
    ("events", EVENTS_SOURCE),
    ("buffer", buffer::BUFFER_SOURCE),
    ("child_process", child_process::CHILD_PROCESS_SOURCE),
    // The console is ONE implementation on two sinks ([`console`]): this module's own, which
    // the runtime binds to the global it installs, and the `Console` a program builds over
    // its own streams.
    ("console", console::CONSOLE_SOURCE),
    ("crypto", crypto::CRYPTO_SOURCE),
    ("fs", fs::FS_SOURCE),
    ("fs/promises", fs::FS_PROMISES_SOURCE),
    ("timers", timers::TIMERS_SOURCE),
    ("timers/promises", timers::TIMERS_PROMISES_SOURCE),
    ("stream", streams::STREAM_SOURCE),
    ("stream/promises", streams::STREAM_PROMISES_SOURCE),
    ("string_decoder", streams::STRING_DECODER_SOURCE),
    ("os", system::OS_SOURCE),
    // The views of what the run already holds ([`environment_views`]): the process object the
    // runtime installs as a global, the two submodules of modules already here, the legacy
    // name of one of them, and the terminal this environment is attached to — none.
    ("process", environment_views::PROCESS_SOURCE),
    ("util/types", environment_views::UTIL_TYPES_SOURCE),
    ("path/posix", environment_views::PATH_POSIX_SOURCE),
    ("sys", environment_views::SYS_SOURCE),
    ("tty", environment_views::TTY_SOURCE),
    // `http` and `https` are ONE client, under each of the two schemes it is imported for
    // ([`http`]): what a request that states no protocol of its own is sent under is the
    // only thing the two modules differ in.
    ("http", http::HTTP_SOURCE),
    ("https", http::HTTP_SOURCE),
    ("assert", system::ASSERT_SOURCE),
    ("assert/strict", system::ASSERT_SOURCE),
    ("querystring", system::QUERYSTRING_SOURCE),
    ("url", system::URL_SOURCE),
    // The socket modules, whose own capability this runtime has none of ([`sockets`]), and
    // the name service it does answer from its own resolver ([`dns`]).
    ("net", sockets::NET_SOURCE),
    ("tls", sockets::TLS_SOURCE),
    ("dns", dns::DNS_SOURCE),
    ("dns/promises", dns::DNS_PROMISES_SOURCE),
    ("zlib", zlib::ZLIB_SOURCE),
    ("worker_threads", worker_threads::WORKER_THREADS_SOURCE),
    ("perf_hooks", diagnostics::PERF_HOOKS_SOURCE),
    ("async_hooks", diagnostics::ASYNC_HOOKS_SOURCE),
    ("v8", diagnostics::V8_SOURCE),
    ("vm", scripting::VM_SOURCE),
    ("readline", scripting::READLINE_SOURCE),
    ("readline/promises", scripting::READLINE_PROMISES_SOURCE),
    // The modules whose whole capability this runtime has none of ([`host_absent`]), supplied
    // for the same reason `child_process` and the socket modules above are: the CALL that
    // needs the capability is refused, never the import.
    ("http2", host_absent::HTTP2_SOURCE),
    ("dgram", host_absent::DGRAM_SOURCE),
    ("cluster", host_absent::CLUSTER_SOURCE),
    ("inspector", host_absent::INSPECTOR_SOURCE),
    ("inspector/promises", host_absent::INSPECTOR_PROMISES_SOURCE),
    ("repl", host_absent::REPL_SOURCE),
    ("wasi", host_absent::WASI_SOURCE),
    ("trace_events", host_absent::TRACE_EVENTS_SOURCE),
];

/// The source of the core module `name` names, or `None` when this emulator supplies none.
///
/// The `node:` prefix a modern import spells a core module with resolves to the same
/// module as the bare name.
#[must_use]
pub(super) fn core_module_source(name: &str) -> Option<&'static str> {
    let bare = name.strip_prefix("node:").unwrap_or(name);
    CORE_MODULES
        .iter()
        .find(|(module, _)| *module == bare)
        .map(|(_, source)| *source)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every core module is reachable under its bare name and under the `node:` name a
    /// modern import spells it with, and nothing else resolves to a module.
    #[test]
    fn a_core_module_resolves_under_both_of_its_names() {
        for (name, _) in CORE_MODULES {
            for spelled in [(*name).to_string(), format!("node:{name}")] {
                assert!(
                    core_module_source(&spelled).is_some(),
                    "case {spelled:?}: expected the core module's source, got none"
                );
            }
        }
        for absent in ["sqlite", "test", "node:sqlite", "", "node:"] {
            assert!(
                core_module_source(absent).is_none(),
                "case {absent:?}: this emulator supplies no such core module, so none must \
                 resolve"
            );
        }
    }
}
