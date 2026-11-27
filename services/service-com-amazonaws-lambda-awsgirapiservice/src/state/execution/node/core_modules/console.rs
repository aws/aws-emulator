// SPDX-License-Identifier: CC0-1.0

//! `console`: the records a node program writes, and the `Console` it builds its own over a
//! pair of streams.
//!
//! A managed `nodejs*` runtime is a promise about an interpreter AND about the standard
//! library that comes with it, so a handler — or a bundle it ships — whose module body says
//! `require('console')` loads on the runtime it was written for, and so it has to here. The
//! same decision stands behind this crate's `child_process` ([`super::child_process`]) and
//! its socket modules ([`super::sockets`]): a module of that standard library is SUPPLIED on
//! the resolver path rather than being absent from it, because an import that fails points
//! the developer at a deployment package that is not at fault. This module is the one a
//! program reaches for to build a logger of its own — `new Console({ stdout: process.stdout
//! })`, the shape a provider function bundles when it wants its own records rather than the
//! runtime's console — so what it has to supply is the `Console` class as well as the names
//! the global console carries.
//!
//! What is here is ONE implementation of the console surface, installed on two sinks:
//!
//! * the GLOBAL console of this runtime, whose records are the ones the runtime frames and
//!   hands the platform ([`super::super::runtime`]'s `writeLine` — the level, the invocation
//!   and the format the function's `LoggingConfig` states are all that writer's);
//! * a `Console` a program CONSTRUCTS, whose records are written to the streams it was
//!   constructed with, which is what node writes them to — `log`, `info`, `debug`, `dir`,
//!   `dirxml`, `table`, `group` and `count` to the output stream, `warn`, `error`, `trace`
//!   and a failed `assert` to the error stream.
//!
//! So `require('console') === console` holds, as it does on node, and a program that calls
//! `console.group`, `console.count`, `console.time` or `console.table` reaches the same
//! implementation whichever console it holds.
//!
//! Everything here is COMPUTATION over what the run already holds: the formatting is the
//! runtime's own ([`::service_common::node_guest_modules`]'s `util.format`), the counters and
//! the group depth are this object's, the elapsed time of a `time`/`timeEnd` pair is read off
//! the one clock this engine has, and the table is drawn from the data the call was given.
//! Nothing here reaches a host file, socket, process or environment, and nothing is written
//! that was not computed from the call's own arguments.

/// `console`: the global console's own surface and the `Console` a program builds over its
/// own streams.
pub(super) const CONSOLE_SOURCE: &str = r##"
'use strict';

var util = require('util');

// Which of the two streams a record goes to, by the LEVEL the method that wrote it carries:
// "console.error() and console.warn() ... print to stderr", and so does the stack a `trace`
// writes; every other method prints to stdout.
var ERROR_LEVELS = { WARN: true, ERROR: true, TRACE: true };

// How far the records written inside one `group` are indented: "prints to stdout ... and
// indents subsequent lines by an additional two spaces".
var GROUP_INDENT = '  ';

// The label a `count`, `time`, `timeLog` or `timeEnd` that names none is kept under
// ("`label` ... Default: 'default'").
var DEFAULT_LABEL = 'default';

// The widths a drawn table pads one cell to: one space on each side of the value.
var CELL_PADDING = 2;

// How many milliseconds make a second and a minute, for the units an elapsed time is stated
// in.
var MS_PER_SECOND = 1000;
var MS_PER_MINUTE = 60000;

// The text a method's arguments make, which is the runtime's own formatting of them: the
// first argument's format specifiers filled from the ones after it, and the remaining values
// inspected.
function formatted(args) {
    return util.format.apply(null, Array.prototype.slice.call(args));
}

// `text` with every line of it indented by `indent` — what a record written inside a group
// carries, so a multi-line value stays inside the group it was written in.
function indented(text, indent) {
    if (indent === '') {
        return String(text);
    }
    return indent + String(text).split('\n').join('\n' + indent);
}

// `character` repeated `count` times.
function repeated(character, count) {
    return count > 0 ? new Array(count + 1).join(character) : '';
}

// ------------------------------------------------------------------ the elapsed time

// How long a `time`/`timeEnd` pair took, in the units node states one in: milliseconds under
// a second, seconds under a minute, and minutes and seconds beyond it.
function elapsedText(milliseconds) {
    if (milliseconds >= MS_PER_MINUTE) {
        var minutes = Math.floor(milliseconds / MS_PER_MINUTE);
        var rest = (milliseconds - (minutes * MS_PER_MINUTE)) / MS_PER_SECOND;
        var seconds = rest.toFixed(3);
        return minutes + ':' + (rest < 10 ? '0' + seconds : seconds) + ' (m:ss.mmm)';
    }
    if (milliseconds >= MS_PER_SECOND) {
        return (milliseconds / MS_PER_SECOND).toFixed(3) + 's';
    }
    return milliseconds.toFixed(3) + 'ms';
}

// ------------------------------------------------------------------ the table

// One cell's text: the value as the runtime inspects it, and nothing at all for a member a
// row does not carry.
function cellText(value) {
    return value === undefined ? '' : util.inspect(value);
}

// The cells of one row: its label, then the member each column names, then the row's own
// value where the table has a `Values` column and this row is not an object.
function rowCells(label, row, names, values) {
    var cells = [String(label)];
    var object = row !== null && typeof row === 'object';
    var at = 0;
    for (at = 0; at < names.length; at += 1) {
        cells.push(object && Object.prototype.hasOwnProperty.call(row, names[at])
            ? cellText(row[names[at]])
            : '');
    }
    if (values) {
        cells.push(object ? '' : cellText(row));
    }
    return cells;
}

// The box one table is drawn in, as node draws it: a rule, the header, a rule, the rows and
// a closing rule, every cell padded to its column's own width.
function drawn(header, rows) {
    var widths = [];
    var at = 0;
    var index = 0;
    for (at = 0; at < header.length; at += 1) {
        widths.push(header[at].length);
    }
    for (index = 0; index < rows.length; index += 1) {
        for (at = 0; at < rows[index].length; at += 1) {
            if (rows[index][at].length > widths[at]) {
                widths[at] = rows[index][at].length;
            }
        }
    }
    var rule = function (left, join, right) {
        var parts = [];
        for (var column = 0; column < widths.length; column += 1) {
            parts.push(repeated('\u2500', widths[column] + CELL_PADDING));
        }
        return left + parts.join(join) + right;
    };
    var row = function (cells) {
        var parts = [];
        for (var column = 0; column < widths.length; column += 1) {
            var cell = cells[column] === undefined ? '' : cells[column];
            parts.push(' ' + cell + repeated(' ', widths[column] - cell.length) + ' ');
        }
        return '\u2502' + parts.join('\u2502') + '\u2502';
    };
    var drawnLines = [
        rule('\u250c', '\u252c', '\u2510'),
        row(header),
        rule('\u251c', '\u253c', '\u2524')
    ];
    for (index = 0; index < rows.length; index += 1) {
        drawnLines.push(row(rows[index]));
    }
    drawnLines.push(rule('\u2514', '\u2534', '\u2518'));
    return drawnLines.join('\n');
}

// The table `console.table(data, columns)` writes, or `null` for data node does not tabulate
// — anything that is not an object — which `table` then logs as it is.
//
// The first column holds the row labels (an array's indices, an object's keys), one column
// per member the rows carry in the order they were first seen, and a `Values` column where a
// row is not an object at all. `columns`, where the call names one, is the set of members the
// table reports.
function tabulated(data, columns) {
    if (data === null || typeof data !== 'object') {
        return null;
    }
    var labels = Object.keys(data);
    var names = [];
    var values = false;
    var at = 0;
    var index = 0;
    for (at = 0; at < labels.length; at += 1) {
        var row = data[labels[at]];
        if (row === null || typeof row !== 'object') {
            values = true;
            continue;
        }
        var members = Object.keys(row);
        for (index = 0; index < members.length; index += 1) {
            if (Array.isArray(columns) && columns.indexOf(members[index]) < 0) {
                continue;
            }
            if (names.indexOf(members[index]) < 0) {
                names.push(members[index]);
            }
        }
    }
    var header = ['(index)'].concat(names);
    if (values) {
        header.push('Values');
    }
    var rows = [];
    for (at = 0; at < labels.length; at += 1) {
        rows.push(rowCells(labels[at], data[labels[at]], names, values));
    }
    return drawn(header, rows);
}

// ------------------------------------------------------------------ the surface

// Install the whole console surface on `target`, writing every record through `write(level,
// args, line)`: the level the method carries, the arguments it was given and the formatted
// line they make at this object's current group depth.
//
// The counters, the timers and that depth are the object's own, so two consoles never share
// one count or one group.
function install(target, write) {
    var indent = '';
    var counts = Object.create(null);
    var started = Object.create(null);

    function record(level, args) {
        write(level, args, indented(formatted(args), indent));
    }

    function line(level, text) {
        write(level, [text], indented(text, indent));
    }

    function labelled(label) {
        return label === undefined ? DEFAULT_LABEL : String(label);
    }

    target.log = function () { record('INFO', arguments); };
    target.info = function () { record('INFO', arguments); };
    target.debug = function () { record('DEBUG', arguments); };
    target.warn = function () { record('WARN', arguments); };
    target.error = function () { record('ERROR', arguments); };
    target.trace = function () { record('TRACE', arguments); };

    // "Uses util.inspect() on obj and prints the resulting string to stdout."
    target.dir = function (value) { line('INFO', util.inspect(value)); };

    // "This method calls console.log() passing it the arguments received."
    target.dirxml = function () { record('INFO', arguments); };

    // "Writes a message and does not otherwise affect execution" — so a failed assertion is
    // reported and never thrown.
    target.assert = function (value) {
        if (value) {
            return;
        }
        var rest = Array.prototype.slice.call(arguments, 1);
        line('ERROR', rest.length === 0
            ? 'Assertion failed'
            : 'Assertion failed: ' + formatted(rest));
    };

    // "Maintains an internal counter specific to label and outputs ... the number of times
    // console.count() has been called with the given label."
    target.count = function (label) {
        var held = labelled(label);
        counts[held] = (counts[held] === undefined ? 0 : counts[held]) + 1;
        line('INFO', held + ': ' + counts[held]);
    };

    // "Resets the internal counter specific to label."
    target.countReset = function (label) {
        var held = labelled(label);
        if (counts[held] === undefined) {
            line('WARN', 'Warning: Count for \'' + held + '\' does not exist');
            return;
        }
        delete counts[held];
    };

    // "Increases indentation of subsequent lines by spaces for groupIndentation length",
    // after printing whatever the call was given.
    target.group = function () {
        if (arguments.length > 0) {
            record('INFO', arguments);
        }
        indent += GROUP_INDENT;
    };

    // "An alias for console.group()."
    target.groupCollapsed = function () { target.group.apply(target, arguments); };

    // "Decreases indentation of subsequent lines" — and never past the outermost level.
    target.groupEnd = function () {
        indent = indent.slice(0, Math.max(0, indent.length - GROUP_INDENT.length));
    };

    // "Starts a timer that can be used to compute the duration of an operation."
    target.time = function (label) {
        var held = labelled(label);
        if (started[held] !== undefined) {
            line('WARN', 'Warning: Label \'' + held + '\' already exists for console.time()');
            return;
        }
        started[held] = Date.now();
    };

    // "Prints the elapsed time and other data arguments to stdout", leaving the timer running.
    target.timeLog = function (label) {
        var held = labelled(label);
        if (started[held] === undefined) {
            line('WARN', 'Warning: No such label \'' + held + '\' for console.timeLog()');
            return;
        }
        var rest = Array.prototype.slice.call(arguments, 1);
        var elapsed = held + ': ' + elapsedText(Date.now() - started[held]);
        line('INFO', rest.length === 0 ? elapsed : elapsed + ' ' + formatted(rest));
    };

    // "Stops a timer that was previously started ... and prints the result to stdout."
    target.timeEnd = function (label) {
        var held = labelled(label);
        if (started[held] === undefined) {
            line('WARN', 'Warning: No such label \'' + held + '\' for console.timeEnd()');
            return;
        }
        var elapsed = Date.now() - started[held];
        delete started[held];
        line('INFO', held + ': ' + elapsedText(elapsed));
    };

    // "Try to construct a table with the columns of the properties of tabularData ... Falls
    // back to just logging the argument if it can't be parsed as tabular."
    target.table = function (data, columns) {
        var table = tabulated(data, columns);
        if (table === null) {
            line('INFO', formatted([data]));
            return;
        }
        line('INFO', table);
    };

    // The methods that act on a TERMINAL or on an attached inspector, neither of which an
    // execution environment has: node's own are no-ops without one, and so are these.
    target.clear = function () { return undefined; };
    target.timeStamp = function () { return undefined; };
    target.profile = function () { return undefined; };
    target.profileEnd = function () { return undefined; };

    return target;
}

// ------------------------------------------------------------------ the Console class

// What node reports for a `Console` built over something that is not a stream.
function notAStream() {
    var error = new TypeError('Console expects a writable stream instance');
    error.code = 'ERR_CONSOLE_WRITABLE_STREAM';
    return error;
}

function isStream(value) {
    return value !== null && value !== undefined && typeof value.write === 'function';
}

// "new Console(options)", and the positional form "new Console(stdout[, stderr])" node still
// accepts: the records of this console are written to the streams it was built with.
function Console(options) {
    if (!(this instanceof Console)) {
        return new Console(options);
    }
    var settings = isStream(options) || arguments.length > 1
        ? { stdout: arguments[0], stderr: arguments[1] }
        : options;
    if (settings === null || typeof settings !== 'object') {
        throw notAStream();
    }
    if (!isStream(settings.stdout)) {
        throw notAStream();
    }
    var out = settings.stdout;
    var failures = isStream(settings.stderr) ? settings.stderr : out;
    install(this, function (level, args, line) {
        (ERROR_LEVELS[level] === true ? failures : out).write(line + '\n');
    });
}

// ------------------------------------------------------------------ what the module is

// The module IS the global console, as it is on node ("the console module exports ... a
// global console instance"), so a program that imports it and one that reads the global hold
// one object — one set of counters, one group depth, one set of timers.
//
// Its records are the RUNTIME's: the writer is installed by
// [`super::super::runtime`] when it binds this module to the global, because the level a
// record carries, the invocation it belongs to and the format it is delivered in are that
// writer's and not this module's. Until then the module is the same surface over a writer
// that records nothing, so a module body that loads this one and writes before the runtime
// has bound it cannot fail.
var recorder = function () { return undefined; };

var exported = install({ Console: Console }, function (level, args, line) {
    recorder(level, args, line);
});

Object.defineProperty(exported, '__recordWith', {
    value: function (write) {
        recorder = write;
        return exported;
    },
    enumerable: false
});

module.exports = exported;
"##;
