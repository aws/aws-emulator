// SPDX-License-Identifier: CC0-1.0

//! The standard-library modules whose whole content is a VIEW of something this execution
//! environment already holds: `process`, `tty`, `util/types`, `path/posix` and `sys`.
//!
//! A managed `nodejs*` runtime is a promise about an interpreter AND about the standard
//! library that comes with it, so a handler — or a bundle it ships — whose module body says
//! `require('process')` or `require('tty')` loads on the runtime it was written for, and so
//! it has to here: a module of that library is SUPPLIED on the resolver path rather than
//! being absent from it, because an import that fails points the developer at a deployment
//! package that is not at fault. The same decision stands behind this crate's
//! `child_process` ([`super::child_process`]), its socket modules ([`super::sockets`]) and
//! its `console` ([`super::console`]).
//!
//! None of these holds anything of its own:
//!
//! * `process` is the process object this runtime installs as a global
//!   ([`super::super::runtime`]), which is what node's own module is — "the process object is
//!   also available ... via require('process')" — so a program that imports it and one that
//!   reads the global hold ONE object;
//! * `util/types` and `path/posix` are members of the two modules that already hold them, as
//!   node's submodules of them are, and `sys` is the legacy alias of `util`;
//! * `tty` answers the one question this environment can answer about a terminal: it has
//!   none, so no descriptor is one, and the stream classes that would hold a terminal refuse
//!   the CALL that would build one rather than the import.
//!
//! Nothing here reaches a host file, socket, process or environment: every answer is read off
//! a value the run already has.

/// `process`: the process object of this execution environment.
pub(super) const PROCESS_SOURCE: &str = r##"
'use strict';

// "The process object is also available ... via require('process')": the module IS the
// global, so a handler that imports it reads the same environment, argument vector and
// streams the global carries.
module.exports = globalThis.process;
"##;

/// `util/types`: the value classification `util` already holds.
pub(super) const UTIL_TYPES_SOURCE: &str = r##"
'use strict';

// "util.types provides type checks for different kinds of built-in objects": the same object
// the `util` module exports as `types`, which is what node's own submodule is.
module.exports = require('util').types;
"##;

/// `path/posix`: the POSIX path algebra `path` already holds, which on this runtime is the
/// whole of it.
pub(super) const PATH_POSIX_SOURCE: &str = r##"
'use strict';

// "The path.posix property provides access to POSIX specific implementations of the path
// methods": the same object the `path` module exports as `posix`. The execution environment
// a function runs in is POSIX, so this is `path` itself.
module.exports = require('path').posix;
"##;

/// `sys`: the legacy name of `util`.
pub(super) const SYS_SOURCE: &str = r##"
'use strict';

// The deprecated alias node still resolves: "the sys module is deprecated. Use util instead."
module.exports = require('util');
"##;

/// `tty`: whether a descriptor of this environment is a terminal, on a runtime attached to
/// none.
pub(super) const TTY_SOURCE: &str = r##"
'use strict';

// What a host answers for an ioctl made on a descriptor that is no terminal, which is what
// every descriptor of an execution environment is: node carries the number negated in
// `errno`, as the result it came from.
var ENOTTY = 25;

// The failure of a call that would hold a terminal, as node builds one.
function notATerminal(syscall) {
    var error = new Error(syscall + ' ENOTTY');
    error.errno = -ENOTTY;
    error.code = 'ENOTTY';
    error.syscall = syscall;
    return error;
}

// "Returns true if fd is associated with a TTY and false if it isn't": an invocation of this
// emulator runs with no terminal attached — its records are delivered to the platform's log
// and never to a console device — so no descriptor of it is one, and none can become one
// while the run lasts.
function isatty() {
    return false;
}

// The two stream classes hold a terminal, which this environment has none of: the call that
// would build one reports the failure node reports when the descriptor it was given is no
// terminal, rather than a stream nothing is attached to.
function ReadStream() {
    throw notATerminal('uv_tty_init');
}

function WriteStream() {
    throw notATerminal('uv_tty_init');
}

module.exports = {
    isatty: isatty,
    ReadStream: ReadStream,
    WriteStream: WriteStream,
};
"##;
