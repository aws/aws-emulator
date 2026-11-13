// SPDX-License-Identifier: CC0-1.0

//! The RUNTIME a JavaScript handler runs under, in the language the handler is written in.
//!
//! A node runtime's job is small and entirely defined by what the handler sees: resolve the
//! handler's module out of the deployment package, build the context object, call the
//! handler with the event and the context, and report back either what it returned or the
//! error document Lambda answers for an exception. That is this source: it runs INSIDE the
//! engine [`super`] builds, because every one of those steps is a JavaScript one — the
//! module system is JavaScript's, `JSON` reads the event and writes the response, and an
//! `Error`'s own `stack` is the trace.
//!
//! What it is NOT is the containment or the bounds. The whole host surface of a run is the
//! three functions [`super::install_host`] installs — the invocation log, a package entry's
//! source, and a core module's source — and this source takes all three off the global
//! object once it holds them. The deadline, the step ceiling and the memory ceiling are the
//! engine's and the host's ([`super::Bounds`]), so no rebinding or deletion of a name here
//! can lift one.
//!
//! The protocol between host and runtime is one frozen global, `__lambdaRuntime`, so the
//! host's own engine surface stays as small as a string in and a string out:
//!
//! * `start(input)` — begins the invocation the JSON document `input` describes: the
//!   handler name, the event, the environment the invocation runs with, and the deadline.
//! * `poll()` — the JSON document of what the invocation came to, once it has, and `null`
//!   while it has not.
//! * `waiting()` — what an unsettled invocation is waiting for: the promise an
//!   asynchronous handler returned, or the empty event loop a non-async handler's answer is
//!   reported at.
//! * `nextTimerDelayMs()` / `runDueTimers()` — the timer queue, which the host drives: it
//!   is the host that holds the clock a run is bounded by, so it is the host that decides
//!   when a timer is due.
//! * `eventLoopEmpty()` — told by the host when no job and no timer is left to run.
//! * `resolveModule(base, name)` / `moduleSource(id)` — how an `import` is resolved and
//!   loaded, which is the same resolution `require` performs: a file of this package, or a
//!   core module this emulator supplies, and nothing else.
//! * `moduleNotFound(name)` — the error a specifier that resolves to nothing fails with,
//!   which the host raises from its own resolver.

/// The JavaScript source of the runtime, evaluated once per invocation in a fresh engine.
pub(super) const RUNTIME_SOURCE: &str = r##"
'use strict';
(function () {
    var TASK_ROOT = '/var/task';

    // The host functions, taken off the global object as soon as they are held: from
    // here on the only way to reach one is through this closure.
    var hostLog = globalThis.__hostLog;
    var hostModuleSource = globalThis.__hostModuleSource;
    var hostModulePaths = globalThis.__hostModulePaths;
    var hostCoreSource = globalThis.__hostCoreSource;
    var hostDigest = globalThis.__hostDigest;
    var hostHmac = globalThis.__hostHmac;
    var hostRandomHex = globalThis.__hostRandomHex;
    var hostAwsCall = globalThis.__hostAwsCall;
    var hostCompileScript = globalThis.__hostCompileScript;
    var hostZoneOffsetMs = globalThis.__hostZoneOffsetMs;
    var hostEncode = globalThis.__hostEncode;
    var hostDecode = globalThis.__hostDecode;
    var hostBeginInvocation = globalThis.__hostBeginInvocation;
    delete globalThis.__hostLog;
    delete globalThis.__hostModuleSource;
    delete globalThis.__hostModulePaths;
    delete globalThis.__hostCoreSource;
    delete globalThis.__hostDigest;
    delete globalThis.__hostHmac;
    delete globalThis.__hostRandomHex;
    delete globalThis.__hostAwsCall;
    delete globalThis.__hostCompileScript;
    delete globalThis.__hostZoneOffsetMs;
    delete globalThis.__hostEncode;
    delete globalThis.__hostDecode;
    delete globalThis.__hostBeginInvocation;

    // Dynamic import is the one way a module of the package is loaded as an ES module. It
    // is compiled here, once, so a run never depends on it being available at any later
    // point.
    var dynamicImport = null;
    try {
        dynamicImport = new Function('specifier', 'return import(specifier);');
    } catch (error) {
        dynamicImport = null;
    }

    var state = null;
    var cache = Object.create(null);
    // The modules a `require` is currently inside, outermost first: the chain a module that
    // cannot be found names as the REQUIRE STACK of the failure ([`requireNotFound`]).
    var loading = [];
    var timers = [];
    var nextTimerId = 1;
    // The major version of the managed runtime this invocation names, which is what the
    // layer search path's own `node<major>` directory is keyed by.
    var nodeMajor = '';

    // ---------------------------------------------------------------- paths

    // The three mounts a function's modules come from, and the id prefix each is keyed
    // under: the deployment package at the task root, a layer's content under the layer
    // root, and the modules the runtime itself supplies.
    var CORE_PREFIX = 'core:';
    var LAYER_PREFIX = 'layer:';
    var RUNTIME_PREFIX = 'runtime:';
    var LAYER_ROOT = '/opt';
    var RUNTIME_ROOT = '/var/runtime';
    // The runtime's OWN entry module, which is what requires the handler's module: the
    // outermost entry of every require stack a failed resolution reports.
    var RUNTIME_ENTRY = RUNTIME_ROOT + '/index.mjs';
    var FILE_URL_PREFIX = 'file://';

    function rootOf(id) {
        if (id.indexOf(LAYER_PREFIX) === 0) {
            return LAYER_PREFIX;
        }
        if (id.indexOf(RUNTIME_PREFIX) === 0) {
            return RUNTIME_PREFIX;
        }
        return '';
    }

    function tailOf(id) {
        return id.slice(rootOf(id).length);
    }

    // The absolute path an id names: what the handler reads as `__filename`, as a module's
    // own url, and as the path in a filesystem error.
    function filenameOf(id) {
        if (id.indexOf(CORE_PREFIX) === 0) {
            return 'node:' + id.slice(CORE_PREFIX.length);
        }
        if (id.indexOf(LAYER_PREFIX) === 0) {
            return LAYER_ROOT + '/' + tailOf(id);
        }
        if (id.indexOf(RUNTIME_PREFIX) === 0) {
            return RUNTIME_ROOT + '/' + tailOf(id);
        }
        return TASK_ROOT + '/' + id;
    }

    // The id an absolute path names, or `null` for a path that is on no mount of this
    // function at all — which is every path of the host.
    function idOfPath(path) {
        var mounts = [
            [TASK_ROOT + '/', ''],
            [LAYER_ROOT + '/', LAYER_PREFIX],
            [RUNTIME_ROOT + '/', RUNTIME_PREFIX]
        ];
        for (var index = 0; index < mounts.length; index += 1) {
            if (path.indexOf(mounts[index][0]) === 0) {
                return packagePath(mounts[index][1], path.slice(mounts[index][0].length));
            }
        }
        return null;
    }

    // The two sources a module is read from, normalised: a host function that holds no
    // such entry answers with nothing, and `null` is what that is here. A path that is not
    // one (a resolution that refused the specifier) is never looked up at all.
    function sourceOf(path) {
        if (typeof path !== 'string' || path === '') {
            return null;
        }
        var source = hostModuleSource(path);
        return source === null || source === undefined ? null : source;
    }

    function coreSourceOf(name) {
        var source = hostCoreSource(name);
        return source === null || source === undefined ? null : source;
    }

    function endsWith(text, suffix) {
        return text.length >= suffix.length
            && text.slice(text.length - suffix.length) === suffix;
    }

    // The directory an id lives in, as an id of the same mount.
    function directoryOf(path) {
        var root = rootOf(path);
        var tail = tailOf(path);
        var cut = tail.lastIndexOf('/');
        return cut < 0 ? root : root + tail.slice(0, cut);
    }

    // The one way a specifier becomes a path INSIDE one of this function's mounts. The
    // segments of `directory` and `request` are collapsed FIRST — `.` dropped, `..`
    // applied — and a path that would leave the mount, or that names no file at all, is
    // refused rather than clamped: no specifier can therefore name anything but an entry
    // of this function's own mounts, whatever `..` or absolute form it is written in.
    // Every resolution below goes through this, so there is no second route to a lookup
    // key.
    function packagePath(directory, request) {
        if (typeof request !== 'string') {
            return null;
        }
        var root = rootOf(directory);
        var held = tailOf(directory);
        var segments = (held === '' ? [] : held.split('/')).concat(request.split('/'));
        var out = [];
        for (var index = 0; index < segments.length; index += 1) {
            var segment = segments[index];
            if (segment === '' || segment === '.') {
                continue;
            }
            if (segment === '..') {
                if (out.length === 0) {
                    return null;
                }
                out.pop();
                continue;
            }
            out.push(segment);
        }
        return out.length === 0 ? null : root + out.join('/');
    }

    // ---------------------------------------------------------------- module resolution

    function coreName(request) {
        var bare = request.indexOf('node:') === 0 ? request.slice(5) : request;
        return coreSourceOf(bare) === null ? null : bare;
    }

    // How many `package.json` "main" entries one resolution may follow. A manifest whose
    // `main` leads back to a directory already being resolved would otherwise make the
    // resolution of a package the caller wrote recurse without end, so the chain is
    // bounded here and a package past the bound resolves to nothing at all.
    var MAX_MANIFEST_HOPS = 8;

    // The package entry `base` names, trying the suffixes a Node resolution tries and
    // then the directory forms. `null` when the package holds no such file.
    function fileCandidate(base) {
        return candidateOf(base, 0);
    }

    function candidateOf(base, hops) {
        var suffixes = ['', '.js', '.json', '.cjs', '.mjs'];
        var index = 0;
        for (index = 0; index < suffixes.length; index += 1) {
            if (sourceOf(base + suffixes[index]) !== null) {
                return base + suffixes[index];
            }
        }
        var manifest = hops < MAX_MANIFEST_HOPS ? sourceOf(base + '/package.json') : null;
        if (manifest !== null) {
            var main = null;
            try {
                main = JSON.parse(manifest).main;
            } catch (error) {
                main = null;
            }
            if (typeof main === 'string' && main !== '') {
                var joined = packagePath(base, main);
                var resolved = joined === null ? null : candidateOf(joined, hops + 1);
                if (resolved !== null) {
                    return resolved;
                }
            }
        }
        var indexes = ['/index.js', '/index.json', '/index.cjs', '/index.mjs'];
        for (index = 0; index < indexes.length; index += 1) {
            if (sourceOf(base + indexes[index]) !== null) {
                return base + indexes[index];
            }
        }
        return null;
    }

    function packageCandidate(directory, request) {
        var current = directory;
        for (;;) {
            var base = packagePath(current, 'node_modules/' + request);
            var found = base === null ? null : fileCandidate(base);
            if (found !== null) {
                return found;
            }
            if (current === rootOf(current)) {
                break;
            }
            current = directoryOf(current);
        }
        // Past the package's own directories come the runtime's search paths, in the order
        // a managed runtime lists them: the layer's directory for this runtime's own major
        // version, the layer's version-independent directory, and the modules the runtime
        // itself supplies. A layer directory naming ANOTHER major version is not on the
        // path at all, so a package shipped only there resolves to nothing.
        var roots = searchRoots();
        for (var index = 0; index < roots.length; index += 1) {
            var candidate = packagePath(roots[index], request);
            var resolved = candidate === null ? null : fileCandidate(candidate);
            if (resolved !== null) {
                return resolved;
            }
        }
        return runtimeSupplied(request);
    }

    // The directories a bare specifier is looked for in once the package's own
    // `node_modules` chain is exhausted.
    function searchRoots() {
        return [
            LAYER_PREFIX + 'nodejs/node' + nodeMajor + '/node_modules',
            LAYER_PREFIX + 'nodejs/node_modules',
            RUNTIME_PREFIX + 'node_modules'
        ];
    }

    // What a module reports as its own `paths`: every `node_modules` directory above it,
    // then the runtime's search paths, as the absolute paths a resolution walks.
    function searchPaths(directory) {
        var out = [];
        var current = directory;
        for (;;) {
            var base = packagePath(current, 'node_modules');
            if (base !== null) {
                out.push(filenameOf(base));
            }
            if (current === rootOf(current)) {
                break;
            }
            current = directoryOf(current);
        }
        var roots = searchRoots();
        for (var index = 0; index < roots.length; index += 1) {
            out.push(filenameOf(roots[index]));
        }
        return out;
    }

    // Resolve `request` as `require` does, from the directory `directory`: a core module
    // this emulator supplies, a file of one of this function's mounts, or nothing. A path
    // of the HOST resolves to nothing, because the only files there are are this
    // function's own — every branch below answers with a [`packagePath`], which is inside
    // a mount or null.
    function resolveRequest(directory, request) {
        if (typeof request !== 'string' || request === '') {
            return null;
        }
        var core = coreName(request);
        if (core !== null) {
            return CORE_PREFIX + core;
        }
        if (request.indexOf(FILE_URL_PREFIX) === 0) {
            // A `file:` URL names a module's own path, which is the form a module of this
            // function is imported by its url under.
            var located = idOfPath(request.slice(FILE_URL_PREFIX.length));
            return located === null ? null : fileCandidate(located);
        }
        if (request.charAt(0) === '.') {
            var relative = packagePath(directory, request);
            return relative === null ? null : fileCandidate(relative);
        }
        if (request.charAt(0) === '/') {
            // An absolute specifier is a path of one of the mounts or it is nothing, and
            // the tail is normalised BEFORE it is looked up, so a `..` in it cannot reach
            // past the root the prefix just established.
            var absolute = idOfPath(request);
            return absolute === null ? null : fileCandidate(absolute);
        }
        return packageCandidate(directory, request);
    }

    // The modules the RUNTIME itself supplies under its own `node_modules`, which a bare
    // specifier reaches once the package's and the layers' own directories are exhausted:
    // the AWS SDK a managed runtime ships.
    //
    // The lookup key is built by [`packagePath`] like every other one, so a specifier is
    // normalised before anything is looked up and one carrying `..` names nothing of this
    // mount: there is no second route to a key.
    function runtimeSupplied(request) {
        var id = packagePath(RUNTIME_PREFIX + 'node_modules', request);
        return id === null || sourceOf(id) === null ? null : id;
    }

    function moduleNotFound(request) {
        var error = new Error("Cannot find module '" + request + "'");
        error.code = 'MODULE_NOT_FOUND';
        return error;
    }

    // The failure a REQUIRE of a specifier that resolves to nothing throws: the message a
    // node runtime writes for it, which names the specifier and then the REQUIRE STACK —
    // the files that were requiring, innermost first, ending at the runtime's own entry
    // module. The stack is what tells a caller WHERE the missing module was asked for, so a
    // packaging error can be told apart from a handler-name one.
    //
    // An `import` that resolves to nothing fails with [`moduleNotFound`] instead: a module
    // the engine's own loader resolves is reached through no require chain, so there is no
    // require stack to name.
    function requireNotFound(request, frames) {
        var stack = frames === undefined || frames === null ? requireStack() : frames;
        var error = new Error(
            "Cannot find module '" + request + "'\nRequire stack:\n- " + stack.join('\n- ')
        );
        error.code = 'MODULE_NOT_FOUND';
        error.requireStack = stack;
        return error;
    }

    // The files a require is currently inside, innermost first, ending at the runtime's own
    // entry module — which is the module that requires the handler's.
    function requireStack() {
        var frames = [];
        for (var index = loading.length - 1; index >= 0; index -= 1) {
            frames.push(loading[index]);
        }
        frames.push(RUNTIME_ENTRY);
        return frames;
    }

    // ---------------------------------------------------------------- module loading

    // Whether the nearest `package.json` above `path` declares its files to be ES
    // modules, which is what `"type": "module"` says.
    function declaredEsModule(path) {
        var directory = directoryOf(path);
        for (;;) {
            var manifest = sourceOf(packagePath(directory, 'package.json'));
            if (manifest !== null) {
                try {
                    return JSON.parse(manifest).type === 'module';
                } catch (error) {
                    return false;
                }
            }
            if (directory === rootOf(directory)) {
                return false;
            }
            directory = directoryOf(directory);
        }
    }

    // Whether a resolved file is loaded as an ES MODULE. The suffix decides when it states
    // one (`.mjs` is a module, `.cjs` is not), and otherwise the nearest manifest's `type`
    // does — which is the whole rule: a `.js` file of a package that does not declare
    // itself a module is CommonJS, so module syntax in it is the syntax error it is.
    function esModuleFile(id) {
        if (endsWith(id, '.mjs')) {
            return true;
        }
        if (endsWith(id, '.cjs') || endsWith(id, '.json')) {
            return false;
        }
        return declaredEsModule(id);
    }

    // The `require` ONE module scope is given: it resolves from that module's own
    // directory, and a specifier it cannot resolve fails naming that module's own require
    // stack — `frames` — rather than whatever happens to be loading when the call is made.
    // A module keeps its `require` after it has loaded, and so keeps the chain it was
    // loaded through, which is the chain node names.
    function requireFrom(directory, frames) {
        var required = function (request) {
            var id = resolveRequest(directory, request);
            if (id === null) {
                throw requireNotFound(request, frames);
            }
            return requireId(id);
        };
        required.resolve = function (request) {
            var id = resolveRequest(directory, request);
            if (id === null) {
                throw requireNotFound(request, frames);
            }
            return filenameOf(id);
        };
        required.cache = cache;
        required.main = undefined;
        required.extensions = { '.js': null, '.json': null, '.node': null };
        return required;
    }

    // The module scope's function, compiled as a unit the engine knows by `name`. A core
    // module is compiled the same way, under the `node:` name it is imported by.
    function compileModule(name, wrapped) {
        var compiled = hostCompileScript(name, wrapped);
        if (typeof compiled !== 'function') {
            throw new Error("Cannot compile module '" + name + "'");
        }
        return compiled;
    }

    // Whether `id` is an id this runtime itself would have produced: a core module's, or a
    // path `packagePath` answers for that id's own mount and tail, byte for byte.
    //
    // Every lookup goes through this FIRST, whatever route reached it. `requireId` is
    // published on the protocol object a module can see (`__lambdaRuntime`), so the
    // collapsing and the mount check `packagePath` performs cannot be the resolution's
    // alone: an id that is not the one `packagePath` answers — one carrying `..`, an
    // absolute host path, anything but a string — names nothing of this function's mounts
    // and is not looked up at all.
    function ownId(id) {
        if (typeof id !== 'string' || id === '') {
            return false;
        }
        if (id.indexOf(CORE_PREFIX) === 0) {
            return true;
        }
        return packagePath(rootOf(id), tailOf(id)) === id;
    }

    function requireId(id) {
        if (!ownId(id)) {
            throw requireNotFound(typeof id === 'string' ? id : '');
        }
        var held = cache[id];
        if (held !== undefined) {
            return held.exports;
        }
        var core = id.indexOf(CORE_PREFIX) === 0;
        var source = core ? coreSourceOf(id.slice(CORE_PREFIX.length)) : sourceOf(id);
        if (source === null) {
            throw requireNotFound(core ? id.slice(CORE_PREFIX.length) : filenameOf(id));
        }
        var filename = filenameOf(id);
        var directory = core ? '' : directoryOf(id);
        var module = {
            id: filename,
            filename: filename,
            exports: {},
            loaded: false,
            children: [],
            paths: core ? [] : searchPaths(directory)
        };
        cache[id] = module;
        // Inside this module from here on: a require it makes that resolves to nothing names
        // this file as the innermost frame of its require stack, over the chain it was itself
        // reached through.
        loading.push(filename);
        var frames = requireStack();
        try {
            if (!core && endsWith(id, '.json')) {
                module.exports = JSON.parse(source);
            } else {
                // The module's own text, wrapped in the function whose parameters are the
                // five names a module scope has. The wrapper's own text is on the FIRST
                // line, so every line of the module keeps the number it has in the file
                // the caller wrote and a stack frame of it names that line.
                //
                // A CORE module is compiled by the engine's own function constructor
                // instead: the name a compilation unit is known by is the name a module of
                // it is IMPORTED by, and a core module's import name is the one its own ES
                // wrapper is declared under ([`requiredWrapper`]), which must stay that
                // wrapper's.
                var compiled = core
                    ? new Function(
                        'exports',
                        'require',
                        'module',
                        '__filename',
                        '__dirname',
                        source + '\n'
                    )
                    : compileModule(
                        filename,
                        'export default function (exports, require, module, __filename, '
                            + '__dirname) {' + source + '\n};\n'
                    );
                compiled.call(
                    module.exports,
                    module.exports,
                    requireFrom(directory, frames),
                    module,
                    filename,
                    core ? 'node:' + id.slice(CORE_PREFIX.length) : filenameOf(directory)
                );
            }
            module.loaded = true;
        } catch (error) {
            delete cache[id];
            throw error;
        } finally {
            loading.pop();
        }
        return module.exports;
    }

    // The source the host declares an ES module from, for an id this runtime resolved.
    ///
    // A file that is ES module source is declared as it was written, with the module's own
    // url set on its `import.meta` — the identity a module resolves its siblings by, which
    // the engine's own loader does not carry. A file that is CommonJS — and a core module —
    // is declared as a module whose default export is what `require` of it produces, which
    // is how a CommonJS module is imported.
    function moduleSource(engineId) {
        var id = idOfEngineName(engineId);
        if (id === null) {
            return null;
        }
        if (id.indexOf(CORE_PREFIX) === 0) {
            return requiredWrapper(id);
        }
        var source = sourceOf(id);
        if (source === null) {
            return null;
        }
        if (endsWith(id, '.json')) {
            return 'export default ' + source + ';\n';
        }
        if (id.indexOf(RUNTIME_PREFIX) === 0) {
            // A package the runtime itself supplies, which on a managed runtime is an ES
            // module of the SDK: its members are its named exports.
            return requiredWrapper(id);
        }
        if (esModuleFile(id)) {
            // Stated on the FIRST line, so every line of the module keeps the number it
            // has in the file the caller wrote and a stack frame of it names that line.
            return 'try { import.meta.url = ' + JSON.stringify(moduleUrl(id))
                + '; } catch (error) { } ' + source;
        }
        return 'const __exports = globalThis.__lambdaRuntime.requireId('
            + JSON.stringify(id) + ');\nexport default __exports;\n';
    }

    // The url a module reports as its own: the `file:` URL of its absolute path, which is
    // what `import.meta.url` carries on a managed runtime.
    function moduleUrl(id) {
        return 'file://' + filenameOf(id);
    }

    // The name the ENGINE knows a module by, which is the name its stack frames carry: a
    // core module as it is imported, and a file by its own url — the two forms a node
    // stack frame names a module with.
    function engineName(id) {
        return id.indexOf(CORE_PREFIX) === 0 ? filenameOf(id) : moduleUrl(id);
    }

    // The id behind an engine module name, or `null` for a name that is of no module of
    // this function at all.
    function idOfEngineName(name) {
        if (typeof name !== 'string') {
            return null;
        }
        if (name.indexOf('node:') === 0) {
            var core = coreName(name);
            return core === null ? null : CORE_PREFIX + core;
        }
        if (name.indexOf(FILE_URL_PREFIX) === 0) {
            return idOfPath(name.slice(FILE_URL_PREFIX.length));
        }
        return null;
    }

    function isIdentifier(name) {
        return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name) && name !== 'default';
    }

    // A module `require` answers with an object for, as an ES MODULE: its own object as the
    // default export, and one named export per member it carries, which is what an `import
    // { member } from` of it needs — the names are bound when the importing module is
    // LINKED, before any of them is read, so they are stated here.
    //
    // This is how a CORE module is imported, and how a module the RUNTIME supplies is: the
    // packages of the AWS SDK a managed node runtime ships, whose client, command and error
    // classes a handler imports by name.
    function requiredWrapper(id) {
        var exported = requireId(id);
        var lines = [
            'const __required = globalThis.__lambdaRuntime.requireId('
                + JSON.stringify(id) + ');',
            'export default __required;'
        ];
        var keys = Object.keys(exported === null || exported === undefined ? {} : exported);
        for (var index = 0; index < keys.length; index += 1) {
            if (isIdentifier(keys[index])) {
                lines.push(
                    'export const ' + keys[index] + ' = __required.' + keys[index] + ';'
                );
            }
        }
        return lines.join('\n') + '\n';
    }

    // Resolve an `import` specifier, through the SAME resolution `require` performs, so an
    // import reaches a file of one of this function's mounts or a core module and nothing
    // else. The base is the importing module's own name as the engine knows it, which is
    // the module's url; an import from anything else resolves from the task root, which is
    // where the runtime's own import of the handler's module starts.
    function resolveModule(base, name) {
        var core = coreName(name);
        if (core !== null) {
            return engineName(CORE_PREFIX + core);
        }
        var baseId = idOfEngineName(base);
        var directory = baseId === null ? '' : directoryOf(baseId);
        var resolved = resolveRequest(directory, name);
        return resolved === null ? null : engineName(resolved);
    }

    // ---------------------------------------------------------------- the console

    var util = requireId('core:util');

    function writeLine(args) {
        hostLog(util.format.apply(null, Array.prototype.slice.call(args)) + '\n');
    }

    var console = {
        log: function () { writeLine(arguments); },
        info: function () { writeLine(arguments); },
        debug: function () { writeLine(arguments); },
        dir: function () { writeLine(arguments); },
        warn: function () { writeLine(arguments); },
        error: function () { writeLine(arguments); },
        trace: function () { writeLine(arguments); }
    };
    globalThis.console = console;

    // ---------------------------------------------------------------- timers

    function timerHandle(timer) {
        return {
            id: timer.id,
            ref: function () { return this; },
            unref: function () { return this; },
            hasRef: function () { return true; }
        };
    }

    function addTimer(callback, delay, args, repeating) {
        if (typeof callback !== 'function') {
            throw new TypeError('The "callback" argument must be of type function.');
        }
        var milliseconds = Number(delay);
        if (!isFinite(milliseconds) || milliseconds < 0) {
            milliseconds = 0;
        }
        var timer = {
            id: nextTimerId,
            due: Date.now() + milliseconds,
            callback: callback,
            args: args,
            every: repeating ? Math.max(1, milliseconds) : null
        };
        nextTimerId += 1;
        timers.push(timer);
        return timerHandle(timer);
    }

    function clearTimer(handle) {
        var id = handle === null || typeof handle !== 'object' ? handle : handle.id;
        for (var index = 0; index < timers.length; index += 1) {
            if (timers[index].id === id) {
                timers.splice(index, 1);
                return;
            }
        }
    }

    function nextTimerDelayMs() {
        if (timers.length === 0) {
            return null;
        }
        var earliest = timers[0].due;
        for (var index = 1; index < timers.length; index += 1) {
            if (timers[index].due < earliest) {
                earliest = timers[index].due;
            }
        }
        return Math.max(0, earliest - Date.now());
    }

    function runDueTimers() {
        var now = Date.now();
        var due = [];
        var remaining = [];
        for (var index = 0; index < timers.length; index += 1) {
            if (timers[index].due <= now) {
                due.push(timers[index]);
                if (timers[index].every !== null) {
                    timers[index].due = now + timers[index].every;
                    remaining.push(timers[index]);
                }
            } else {
                remaining.push(timers[index]);
            }
        }
        timers = remaining;
        for (var ran = 0; ran < due.length; ran += 1) {
            try {
                due[ran].callback.apply(undefined, due[ran].args);
            } catch (error) {
                // An exception nothing caught ends the invocation, as it does on the real
                // runtime: the handler's work is over whatever is still queued.
                settle(raised(error));
                return;
            }
        }
    }

    globalThis.setTimeout = function (callback, delay) {
        return addTimer(callback, delay, Array.prototype.slice.call(arguments, 2), false);
    };
    globalThis.setInterval = function (callback, delay) {
        return addTimer(callback, delay, Array.prototype.slice.call(arguments, 2), true);
    };
    globalThis.setImmediate = function (callback) {
        return addTimer(callback, 0, Array.prototype.slice.call(arguments, 1), false);
    };
    globalThis.clearTimeout = clearTimer;
    globalThis.clearInterval = clearTimer;
    globalThis.clearImmediate = clearTimer;
    if (typeof globalThis.queueMicrotask !== 'function') {
        globalThis.queueMicrotask = function (callback) {
            Promise.resolve().then(callback);
        };
    }

    // ---------------------------------------------------------------- the globals

    // The globals a managed runtime defines beyond the language's own. Each is taken from
    // the core module that implements it, so a handler that imports the module and one
    // that reads the global hold the same object.
    function installGlobals() {
        var buffer = requireId(CORE_PREFIX + 'buffer');
        globalThis.Buffer = buffer.Buffer;
        globalThis.atob = buffer.atob;
        globalThis.btoa = buffer.btoa;
        var url = requireId(CORE_PREFIX + 'url');
        globalThis.URL = url.URL;
        globalThis.URLSearchParams = url.URLSearchParams;
        globalThis.crypto = requireId(CORE_PREFIX + 'crypto').webcrypto;
        if (typeof globalThis.TextEncoder !== 'function') {
            globalThis.TextEncoder = function TextEncoder() { this.encoding = 'utf-8'; };
            globalThis.TextEncoder.prototype.encode = function (text) {
                var held = buffer.Buffer.from(text === undefined ? '' : String(text), 'utf8');
                return new Uint8Array(held);
            };
        }
        if (typeof globalThis.TextDecoder !== 'function') {
            globalThis.TextDecoder = function TextDecoder(encoding) {
                this.encoding = encoding === undefined ? 'utf-8' : String(encoding);
            };
            globalThis.TextDecoder.prototype.decode = function (bytes) {
                return bytes === undefined || bytes === null
                    ? ''
                    : buffer.Buffer.from(bytes).toString(this.encoding);
            };
        }
        installAbort();
        // "Error.stackTraceLimit ... The default value is 10": the number of frames an
        // error's own stack carries, which `NODE_OPTIONS` can set.
        Error.stackTraceLimit = 10;
    }

    // The abort pair a cancellable asynchronous call is given: a signal with the one event
    // it fires, and the controller that fires it.
    function installAbort() {
        if (typeof globalThis.AbortController === 'function') {
            return;
        }
        var AbortSignal = function AbortSignal() {
            this.aborted = false;
            this.reason = undefined;
            this._listeners = [];
        };
        AbortSignal.prototype.addEventListener = function (event, listener) {
            if (event !== 'abort') {
                return;
            }
            if (this.aborted) {
                listener({ type: 'abort', target: this });
                return;
            }
            this._listeners.push(listener);
        };
        AbortSignal.prototype.removeEventListener = function (event, listener) {
            this._listeners = this._listeners.filter(function (held) { return held !== listener; });
        };
        AbortSignal.prototype.throwIfAborted = function () {
            if (this.aborted) {
                throw this.reason;
            }
        };
        var AbortController = function AbortController() {
            this.signal = new AbortSignal();
        };
        AbortController.prototype.abort = function (reason) {
            if (this.signal.aborted) {
                return;
            }
            this.signal.aborted = true;
            if (reason === undefined) {
                reason = new Error('This operation was aborted');
                reason.name = 'AbortError';
                reason.code = 'ABORT_ERR';
            }
            this.signal.reason = reason;
            var listeners = this.signal._listeners;
            this.signal._listeners = [];
            for (var index = 0; index < listeners.length; index += 1) {
                listeners[index]({ type: 'abort', target: this.signal });
            }
        };
        globalThis.AbortController = AbortController;
        globalThis.AbortSignal = AbortSignal;
    }

    // ---------------------------------------------------------------- the process object

    function installProcess(input) {
        var EventEmitter = requireId(CORE_PREFIX + 'events');
        var process = new EventEmitter();
        var startedAt = Date.now();
        process.env = input.environment;
        process.argv = ['/var/lang/bin/node', RUNTIME_ROOT + '/index.mjs'];
        process.argv0 = 'node';
        process.execPath = '/var/lang/bin/node';
        // The platform and the architecture this process really runs on, and the Node
        // release line the function's own `Runtime` names.
        process.platform = input.platform;
        process.arch = input.arch;
        process.version = 'v' + input.nodeVersion;
        process.versions = { node: input.nodeVersion };
        process.release = { name: 'node' };
        process.exitCode = undefined;
        process.cwd = function () { return TASK_ROOT; };
        process.uptime = function () { return (Date.now() - startedAt) / 1000; };
        process.hrtime = function (previous) {
            var nanoseconds = Math.round(Date.now() * 1e6);
            var seconds = Math.floor(nanoseconds / 1e9);
            var rest = nanoseconds - (seconds * 1e9);
            if (previous === undefined) {
                return [seconds, rest];
            }
            var elapsed = ((seconds - previous[0]) * 1e9) + (rest - previous[1]);
            return [Math.floor(elapsed / 1e9), elapsed % 1e9];
        };
        process.hrtime.bigint = function () {
            return BigInt(Math.round(Date.now())) * BigInt(1e6);
        };
        process.nextTick = function (callback) {
            var args = Array.prototype.slice.call(arguments, 1);
            Promise.resolve().then(function () { callback.apply(undefined, args); });
        };
        process.exit = function () {
            // "Runtime exited without providing a reason" is what Lambda reports for a
            // runtime that ended the process instead of answering the invocation.
            settle({
                outcome: 'raised',
                errorType: 'Runtime.ExitError',
                errorMessage: 'Runtime exited without providing a reason',
                trace: []
            });
            throw new Error('Runtime exited without providing a reason');
        };
        process.emitWarning = function (warning) {
            hostLog('Warning: ' + (warning === null || warning === undefined
                ? '' : String(warning.message === undefined ? warning : warning.message)) + '\n');
        };
        process.stdout = {
            write: function (text) { hostLog(String(text)); return true; },
            isTTY: false,
            fd: 1
        };
        process.stderr = {
            write: function (text) { hostLog(String(text)); return true; },
            isTTY: false,
            fd: 2
        };
        globalThis.process = process;
    }

    // ---------------------------------------------------------------- the runtime options

    // What `NODE_OPTIONS` may carry. Two options this runtime APPLIES — a module to preload
    // before the handler's own is loaded, and the depth of an error's stack. Everything
    // else node accepts there sizes, quiets or instruments the process, which this engine's
    // own ceilings and log already decide, so such an option is accepted and has no effect;
    // those are recognised by the FAMILY their name belongs to rather than one spelling at a
    // time. An option belonging to no family is one this runtime does not know, and node
    // itself refuses an option it does not know: it does not start, so the invocation
    // reports the runtime's exit and no handler runs.
    var NODE_OPTIONS_APPLIED = ['--require', '-r', '--stack-trace-limit'];
    var NODE_OPTIONS_FAMILIES = [
        '--experimental-',
        '--trace-',
        '--no-',
        '--max-',
        '--min-',
        '--report-',
        '--heapsnapshot-',
        '--heap-',
        '--dns-',
        '--tls-',
        '--use-',
        '--preserve-symlinks',
        '--pending-deprecation',
        '--throw-deprecation',
        '--enable-',
        '--disable-',
        '--force-',
        '--unhandled-rejections',
        '--icu-data-dir',
        '--conditions',
        '--loader',
        '--import',
        '--input-type',
        '--network-family-autoselection',
        '--openssl-',
        '--secure-heap',
        '--snapshot-blob',
        '--title',
        '--zero-fill-buffers',
        '--v8-pool-size',
        '--perf-',
        '--prof',
        '--cpu-prof',
        '--redirect-warnings',
        '--watch'
    ];

    function knownNodeOption(flag) {
        if (NODE_OPTIONS_APPLIED.indexOf(flag) >= 0) {
            return true;
        }
        for (var index = 0; index < NODE_OPTIONS_FAMILIES.length; index += 1) {
            if (flag.indexOf(NODE_OPTIONS_FAMILIES[index]) === 0) {
                return true;
            }
        }
        return false;
    }

    function exited(message) {
        // The error type the managed node runtime reports for a NODE PROCESS that ended
        // instead of answering the invocation — a runtime option it would not start with, or
        // an event loop that emptied with the handler's promise still pending. It is a
        // different condition from a runtime that ended ITSELF while answering
        // (`process.exit`), which is reported as the exit error that names a reason.
        return runtimeFailure('Runtime.NodeJsExit', message);
    }

    // Apply `NODE_OPTIONS`, or report the exit a bad option makes. `null` is applied.
    function applyNodeOptions(options) {
        if (typeof options !== 'string' || options.trim() === '') {
            return null;
        }
        var words = options.trim().split(/\s+/);
        var preload = [];
        for (var index = 0; index < words.length; index += 1) {
            var word = words[index];
            var cut = word.indexOf('=');
            var flag = cut < 0 ? word : word.slice(0, cut);
            var value = cut < 0 ? null : word.slice(cut + 1);
            if (!knownNodeOption(flag)) {
                return exited('node: bad option: ' + flag);
            }
            if (value === null && (flag === '--require' || flag === '-r'
                || flag === '--stack-trace-limit')) {
                index += 1;
                value = index < words.length ? words[index] : null;
                if (value === null) {
                    return exited('node: ' + flag + ' requires an argument');
                }
            }
            if (flag === '--require' || flag === '-r') {
                preload.push(value);
            } else if (flag === '--stack-trace-limit') {
                var depth = Number(value);
                if (isFinite(depth) && depth >= 0) {
                    Error.stackTraceLimit = depth;
                }
            }
        }
        for (var at = 0; at < preload.length; at += 1) {
            var id = resolveRequest('', preload[at]);
            if (id === null) {
                return exited("Cannot find module '" + preload[at] + "'");
            }
            try {
                requireId(id);
            } catch (error) {
                return raised(error);
            }
        }
        return null;
    }

    // ---------------------------------------------------------------- the invocation

    // The LINES of one error's stack text, which is what a node runtime reports a trace as:
    // the stack is text, and the document carries it one entry per line, so no entry ever
    // holds a newline. A head line whose message spans several lines (a `Cannot find module`
    // message carries its require stack) therefore occupies one entry per line of it.
    function traceLines(head) {
        return String(head).split('\n');
    }

    // What a node runtime reports for a value a handler threw. An ERROR reports its own
    // class, its message and its stack — whose first line is the "<ErrorType>: <message>"
    // line, which this engine's stack does not carry, so it is stated here. Anything else
    // reports the TYPE of the value and the value as text, with no trace at all, because a
    // value that is not an error carries no stack to report.
    function raised(error) {
        if (!(error instanceof Error)) {
            return {
                outcome: 'raised',
                errorType: typeof error,
                errorMessage: String(error),
                trace: []
            };
        }
        var type = typeof error.name === 'string' && error.name !== '' ? error.name : 'Error';
        var message = error.message === undefined ? '' : String(error.message);
        var trace = traceLines(type + ': ' + decorated(message));
        if (typeof error.stack === 'string' && error.stack !== '') {
            var frames = error.stack.split('\n');
            for (var index = 0; index < frames.length; index += 1) {
                if (frames[index] !== '') {
                    trace.push(frames[index]);
                }
            }
        }
        return {
            outcome: 'raised',
            errorType: type,
            errorMessage: decorated(message),
            trace: trace
        };
    }

    // How many entries of a reported trace its HEAD line occupies: one per line of the
    // "<ErrorType>: <message>" line a trace opens with ([`traceLines`]).
    function headLineCount(reported) {
        return traceLines(reported.errorType + ': ' + reported.errorMessage).length;
    }

    // The message the RUNTIME reports for an error class whose own wording it states rather
    // than the engine's. Four of them:
    //
    // * `require` is not a binding of ES module scope, and a module that reaches for it is
    //   told which of the two module systems it is in and what to use instead;
    // * a reserved word used where the module system does not accept it names the token it
    //   found, which is how a node runtime reports a syntax error of that class;
    // * a property READ on `undefined` or `null` is reported "Cannot read properties of
    //   <undefined|null> (reading '<name>')", and a property WRITE on one "Cannot set
    //   properties of <undefined|null> (setting '<name>')" — the wording the JavaScript
    //   engine a `nodejs` runtime embeds states for that `TypeError`, which is the text a
    //   client reads in the function's log and in the error document the invocation answers.
    //   The class, the property name and the value read are the engine's own; only the
    //   wording is the runtime's.
    function decorated(message) {
        if (message === 'require is not defined') {
            var told = 'require is not defined in ES module scope, you can use import instead';
            return state !== null && state.esModuleHint !== null && state.esModuleHint !== undefined
                ? told + '\n' + state.esModuleHint
                : told;
        }
        var keyword = /^unsupported keyword: ([A-Za-z]+)$/.exec(message);
        if (keyword !== null) {
            return "Unexpected token '" + keyword[1] + "'";
        }
        var read = /^cannot read property '(.*)' of (undefined|null)$/.exec(message);
        if (read !== null) {
            return 'Cannot read properties of ' + read[2] + " (reading '" + read[1] + "')";
        }
        var written = /^cannot set property '(.*)' of (undefined|null)$/.exec(message);
        if (written !== null) {
            return 'Cannot set properties of ' + written[2] + " (setting '" + written[1] + "')";
        }
        return message;
    }

    // Why the module that is running is being treated as an ES module, when that is the
    // nearest manifest's doing rather than the file's own suffix: the sentence a node
    // runtime adds to the error of a module that reached for a CommonJS binding, naming the
    // manifest that decided it and the suffix that would undo it.
    function esModuleHint(id) {
        if (endsWith(id, '.mjs')) {
            return null;
        }
        var manifest = declaringManifest(id);
        if (manifest === null) {
            return null;
        }
        var cut = id.lastIndexOf('.');
        var suffix = cut < 0 ? '' : id.slice(cut);
        return "This file is being treated as an ES module because it has a '" + suffix
            + "' file extension and '" + filenameOf(manifest)
            + '\' contains "type": "module". To treat it as a CommonJS script, rename it to '
            + "use the '.cjs' file extension.";
    }

    // The manifest that declared the files above `path` to be ES modules, or `null` when
    // none did.
    function declaringManifest(path) {
        var directory = directoryOf(path);
        for (;;) {
            var candidate = packagePath(directory, 'package.json');
            var manifest = sourceOf(candidate);
            if (manifest !== null) {
                try {
                    return JSON.parse(manifest).type === 'module' ? candidate : null;
                } catch (error) {
                    return null;
                }
            }
            if (directory === rootOf(directory)) {
                return null;
            }
            directory = directoryOf(directory);
        }
    }

    // What the runtime reports for a value its response serializer cannot turn into JSON;
    // see [`serialized`].
    var UNSTRINGIFIABLE_RESPONSE = 'Unable to stringify response body';

    // A failure of the RUNTIME rather than of a value the handler threw. Its trace is the
    // one line an error with no stack has — the "<ErrorType>: <message>" line every node
    // trace starts with.
    function runtimeFailure(type, message) {
        return {
            outcome: 'raised',
            errorType: type,
            errorMessage: message,
            trace: traceLines(type + ': ' + message)
        };
    }

    // The record a node runtime writes for a failure of the handler's own: the invocation's
    // id, the ERROR level and the error document, which is where a client reads what a
    // failed invocation came to. Written once per invocation, because one invocation fails
    // once.
    function logInvokeError(reported) {
        if (state === null || state.loggedError) {
            return;
        }
        state.loggedError = true;
        hostLog(new Date().toISOString() + '\t' + state.requestId + '\tERROR\tInvoke Error \t'
            + JSON.stringify({
                errorType: reported.errorType,
                errorMessage: reported.errorMessage,
                stack: reported.trace
            }) + '\n');
    }

    // A value the HANDLER failed with: reported as the error it is, and recorded in the
    // invocation's log the way a node runtime records one.
    function handlerFailure(error) {
        var reported = raised(error);
        logInvokeError(reported);
        return reported;
    }

    // ---------------------------------------------------------------- rejections

    // Told by the engine of every promise rejection that had nothing to handle it, and of
    // every one that was handled after the fact. A rejection is only a FAILURE once the job
    // queue has drained with nothing having handled it, which is what the host asks about
    // ([`reportUnhandledRejection`]); until then it is only pending.
    function rejectionTracked(promise, reason, handled) {
        if (state === null || state.settled) {
            return;
        }
        if (handled) {
            var kept = [];
            for (var index = 0; index < state.rejections.length; index += 1) {
                if (state.rejections[index].promise !== promise) {
                    kept.push(state.rejections[index]);
                }
            }
            state.rejections = kept;
            return;
        }
        state.rejections.push({ promise: promise, reason: reason });
    }

    // What a node runtime reports for a rejection nothing handled: the runtime's own error
    // class, carrying the rejection reason as the message and the reason's own stack.
    function unhandledRejection(reason) {
        var reported = raised(reason);
        var message = reason instanceof Error
            ? reported.errorType + ': ' + reported.errorMessage
            : String(reason);
        return {
            outcome: 'raised',
            errorType: 'Runtime.UnhandledPromiseRejection',
            errorMessage: message,
            // The runtime's own error line, then the frames of the rejection reason's own
            // stack under it: the reason's head line is replaced rather than repeated,
            // because the error being reported is the runtime's.
            trace: traceLines('Runtime.UnhandledPromiseRejection: ' + message)
                .concat(reported.trace.slice(headLineCount(reported)))
        };
    }

    function reportUnhandledRejection() {
        if (state === null || state.settled || state.rejections.length === 0) {
            return;
        }
        var pending = state.rejections.shift();
        var reported = unhandledRejection(pending.reason);
        logInvokeError(reported);
        settle(reported);
    }

    // The response the runtime answers for a value the handler produced: that value as
    // JSON text.
    //
    // A value the serializer cannot turn into JSON fails the invocation as the RUNTIME's own
    // `Error` — "Unable to stringify response body" — rather than as the class the engine
    // raised for the value it refused (a circular structure, a BigInt): what failed is the
    // runtime's attempt to answer with that value, so that is the error reported, and it is
    // recorded in the invocation's log the way every failed invocation is.
    function serialized(value) {
        var payload;
        try {
            payload = JSON.stringify(value === undefined ? null : value);
        } catch (error) {
            return handlerFailure(new Error(UNSTRINGIFIABLE_RESPONSE));
        }
        if (payload === undefined) {
            payload = 'null';
        }
        return { outcome: 'returned', payload: payload };
    }

    function settle(outcome) {
        if (state === null || state.settled) {
            return;
        }
        // A rejection nothing handled ends the node process, so it is the invocation's
        // answer even where the handler itself went on to produce one: the process does not
        // survive to deliver that answer.
        var answered = outcome;
        if (answered.outcome !== 'raised' && state.rejections.length > 0) {
            answered = unhandledRejection(state.rejections.shift().reason);
            logInvokeError(answered);
        }
        state.settled = true;
        try {
            state.result = JSON.stringify(answered);
        } catch (error) {
            state.result = JSON.stringify(
                runtimeFailure('Runtime.MarshalError', 'Unable to marshal response: ' + error)
            );
        }
    }

    // The handler string names the module to load and the property path to the function
    // on it: everything up to the first `.` is the module, the rest addresses the export.
    function splitHandler(handler) {
        if (typeof handler !== 'string') {
            return null;
        }
        var cut = handler.indexOf('.');
        if (cut <= 0 || cut === handler.length - 1) {
            return null;
        }
        return { module: handler.slice(0, cut), path: handler.slice(cut + 1).split('.') };
    }

    function property(exported, path) {
        var held = exported;
        for (var index = 0; index < path.length; index += 1) {
            if (held === null || held === undefined) {
                return undefined;
            }
            held = held[path[index]];
        }
        return held;
    }

    function loadHandlerModule(modulePath) {
        // The module the `Handler` names is a path inside the package like any other, so
        // it is normalised and contained the same way before anything is looked up.
        var inside = packagePath('', modulePath);
        var id = inside === null ? null : fileCandidate(inside);
        if (id === null) {
            return Promise.reject(requireNotFound(modulePath));
        }
        if (esModuleFile(id)) {
            // Why this file is an ES module, for the error of a module that reaches for a
            // CommonJS binding it therefore does not have.
            state.esModuleHint = esModuleHint(id);
            return importModule(id);
        }
        try {
            return Promise.resolve(requireId(id));
        } catch (error) {
            return Promise.reject(error);
        }
    }

    function importModule(id) {
        if (dynamicImport === null) {
            return Promise.reject(new Error("Cannot load ES module '" + id + "'"));
        }
        return dynamicImport(engineName(id));
    }

    function makeContext(input) {
        var environment = input.environment;
        var context = {
            callbackWaitsForEmptyEventLoop: true,
            functionName: environment.AWS_LAMBDA_FUNCTION_NAME,
            functionVersion: environment.AWS_LAMBDA_FUNCTION_VERSION,
            invokedFunctionArn: input.invokedFunctionArn,
            memoryLimitInMB: environment.AWS_LAMBDA_FUNCTION_MEMORY_SIZE,
            awsRequestId: input.requestId,
            logGroupName: environment.AWS_LAMBDA_LOG_GROUP_NAME,
            logStreamName: environment.AWS_LAMBDA_LOG_STREAM_NAME,
            identity: undefined,
            clientContext: input.clientContext === null ? undefined : input.clientContext,
            tenantId: input.tenantId === null ? undefined : input.tenantId,
            getRemainingTimeInMillis: function () {
                return Math.max(0, Math.floor(input.deadlineEpochMs - Date.now()));
            }
        };
        // The legacy completion API a node context carries beside the callback:
        // "context.succeed(), context.fail() and context.done() ... are the non-async
        // completion methods". Each ENDS the invocation where it is called, with the value
        // or the error it was given, rather than waiting for the event loop.
        context.succeed = function (value) {
            settle(serialized(value));
        };
        context.fail = function (error) {
            settle(error === null || error === undefined
                ? serialized(null)
                : handlerFailure(error));
        };
        context.done = function (error, value) {
            if (error === null || error === undefined) {
                context.succeed(value);
                return;
            }
            context.fail(error);
        };
        return context;
    }

    // ---------------------------------------------------------------- response streaming

    // "awslambda.streamifyResponse" marks a handler as one the runtime calls with a
    // RESPONSE STREAM: its answer is the bytes it writes, in the order it writes them,
    // rather than a value it returns. The mark is the well-known symbol the runtime reads
    // it by.
    var STREAMING_MARK = Symbol.for('aws.lambda.runtime.handler.streaming');
    var RESPONSE_STREAMING = 'response';
    // "HttpResponseStream.from" announces the response as the http integration one, whose
    // prelude is the metadata document followed by the eight NUL bytes that end it.
    var HTTP_INTEGRATION_CONTENT_TYPE = 'application/vnd.awslambda.http-integration-response';
    var PRELUDE_TERMINATOR = '\u0000\u0000\u0000\u0000\u0000\u0000\u0000\u0000';

    function installAwsLambda() {
        globalThis.awslambda = Object.freeze({
            streamifyResponse: function (handler) {
                if (typeof handler !== 'function') {
                    throw new TypeError('The handler must be a function');
                }
                handler[STREAMING_MARK] = RESPONSE_STREAMING;
                return handler;
            },
            HttpResponseStream: Object.freeze({
                from: function (responseStream, metadata) {
                    responseStream.setContentType(HTTP_INTEGRATION_CONTENT_TYPE);
                    responseStream.write(JSON.stringify(
                        metadata === undefined || metadata === null ? {} : metadata
                    ));
                    responseStream.write(PRELUDE_TERMINATOR);
                    return responseStream;
                }
            })
        });
    }

    // The text of one write. A string is written as it is; bytes are written as the text
    // they encode, which is how a response stream carries them.
    function writtenText(chunk, encoding) {
        if (typeof chunk === 'string') {
            return chunk;
        }
        if (chunk === null || chunk === undefined) {
            return '';
        }
        return globalThis.Buffer.from(chunk).toString(
            encoding === undefined || encoding === null || typeof encoding !== 'string'
                ? 'utf8'
                : encoding
        );
    }

    // The stream a response-streaming handler writes its answer to: a writable sink whose
    // every write is kept, in order, as one chunk of the response.
    function responseStream(chunks) {
        var Writable = requireId(CORE_PREFIX + 'stream').Writable;
        var stream = new Writable({
            write: function (chunk, encoding, done) {
                var text = writtenText(chunk, encoding);
                if (text !== '') {
                    chunks.push(text);
                }
                done();
            }
        });
        stream.setContentType = function (type) {
            stream.contentType = String(type);
        };
        return stream;
    }

    function callStreamingHandler(entry, event, context, stream, chunks) {
        state.waiting = 'promise';
        var streamed = function () {
            return { outcome: 'streamed', chunks: chunks };
        };
        return Promise.resolve(entry(event, stream, context)).then(streamed, function (error) {
            // A response-streaming invocation's answer is the stream, and a stream that has
            // been begun cannot be withdrawn: what the caller is delivered is the bytes the
            // handler wrote, and the failure is reported BESIDE them, so the stream's
            // trailer can name the error that cut the response short. The failure is also
            // in the invocation's LOG — the `Invoke Error` record below — which is where a
            // streaming client reads what a run came to. A handler that failed before its
            // first write delivers no bytes and no error code: the stream was never begun,
            // so there is nothing for an error to have truncated, and its failure is in the
            // log alone.
            var reported = raised(error);
            logInvokeError(reported);
            if (chunks.length === 0) {
                return streamed();
            }
            return {
                outcome: 'streamed',
                chunks: chunks,
                errorType: reported.errorType,
                errorMessage: reported.errorMessage,
                trace: reported.trace
            };
        });
    }


    function finishFromCallback() {
        if (state.callbackError !== null && state.callbackError !== undefined) {
            settle(handlerFailure(state.callbackError));
            return;
        }
        settle(serialized(state.callbackValue));
    }

    function callHandler(entry, event, context) {
        if (entry[STREAMING_MARK] === RESPONSE_STREAMING) {
            var chunks = [];
            return callStreamingHandler(entry, event, context, responseStream(chunks), chunks);
        }
        var callback = function (error, value) {
            if (state.callbackCalled) {
                return;
            }
            state.callbackCalled = true;
            state.callbackError = error;
            state.callbackValue = value;
            // "callbackWaitsForEmptyEventLoop ... Set to false to send the response right
            // away": otherwise the answer is reported when the event loop is empty.
            if (context.callbackWaitsForEmptyEventLoop === false) {
                finishFromCallback();
            }
        };
        var returned = entry(event, context, callback);
        if (returned !== null && returned !== undefined && typeof returned.then === 'function') {
            // An asynchronous handler's answer is its promise's: the invocation ends when
            // that promise settles.
            state.waiting = 'promise';
            return Promise.resolve(returned).then(serialized, handlerFailure);
        }
        // A non-async handler answers through its callback, and the runtime reports that
        // answer once the event loop is empty — `null` when the callback was never called.
        state.waiting = 'loop';
        return new Promise(function () { return undefined; });
    }

    function invoke(input) {
        var split = splitHandler(input.handler);
        if (split === null) {
            return Promise.resolve(
                runtimeFailure('Runtime.MalformedHandlerName', "Bad handler '" + input.handler + "'")
            );
        }
        return loadHandlerModule(split.module).then(function (exported) {
            var entry = property(exported, split.path);
            // The two ways the handler string names nothing callable, each reported the
            // way a node runtime words it: an export the module does not have at all, and
            // an export it has that is not a function.
            if (entry === null || entry === undefined) {
                return runtimeFailure(
                    'Runtime.HandlerNotFound',
                    input.handler + ' is undefined or not exported'
                );
            }
            if (typeof entry !== 'function') {
                return runtimeFailure(
                    'Runtime.HandlerNotFound',
                    input.handler + ' is not a function'
                );
            }
            var event;
            try {
                event = input.event === null || input.event === undefined || input.event === ''
                    ? {}
                    : JSON.parse(input.event);
            } catch (error) {
                return runtimeFailure('Runtime.InvalidEvent', String(error));
            }
            // The INIT phase is over here: the engine, this runtime and the handler's own
            // module are all loaded and its entry point is resolved. None of that is the
            // handler's run, which is what the function's `Timeout` bounds, so the host is
            // told the invoke phase begins now and answers the instant that `Timeout` is up
            // at — which is what the context reports as the time remaining.
            input.deadlineEpochMs = hostBeginInvocation();
            return callHandler(entry, event, makeContext(input));
        }, function (error) {
            // How the node runtime classifies a module that would not load: a module it
            // could not find, source that would not compile, and anything else the
            // module's own top-level code threw — which is reported as itself.
            if (error !== null && error !== undefined && error.code === 'MODULE_NOT_FOUND') {
                return runtimeFailure(
                    'Runtime.ImportModuleError',
                    'Error: ' + error.message
                );
            }
            if (error instanceof SyntaxError) {
                return runtimeFailure(
                    'Runtime.UserCodeSyntaxError',
                    'SyntaxError: ' + decorated(error.message)
                );
            }
            return raised(error);
        });
    }

    function start(inputJson) {
        var input = JSON.parse(inputJson);
        state = {
            settled: false,
            result: null,
            waiting: 'promise',
            callbackCalled: false,
            callbackError: null,
            callbackValue: null,
            requestId: input.requestId,
            esModuleHint: null,
            loggedError: false,
            rejections: []
        };
        nodeMajor = input.nodeMajor;
        installProcess(input);
        installGlobals();
        installAwsLambda();
        // The runtime's own options are applied before any module of the package is
        // loaded, because one of them decides which module is loaded first.
        var refused = applyNodeOptions(input.environment.NODE_OPTIONS);
        if (refused !== null) {
            settle(refused);
            return;
        }
        Promise.resolve().then(function () {
            return invoke(input);
        }).then(function (outcome) {
            settle(outcome);
        }, function (error) {
            settle(raised(error));
        });
    }

    function eventLoopEmpty() {
        if (state === null || state.settled || state.waiting !== 'loop') {
            return;
        }
        finishFromCallback();
    }

    // Told by the host when the invocation is still unsettled and NOTHING is left that
    // could settle it: no job, no timer, and a handler whose promise is pending. That is
    // the point a node process has nothing left to run and exits, so the invocation is
    // reported as the runtime having exited rather than as the handler having answered.
    function eventLoopExhausted() {
        if (state === null || state.settled) {
            return;
        }
        settle(exited('Node.js process exited before the invocation was answered'));
    }

    globalThis.__lambdaRuntime = Object.freeze({
        start: start,
        poll: function () {
            return state !== null && state.settled ? state.result : null;
        },
        waiting: function () {
            return state === null || state.settled ? '' : state.waiting;
        },
        nextTimerDelayMs: nextTimerDelayMs,
        runDueTimers: runDueTimers,
        eventLoopEmpty: eventLoopEmpty,
        eventLoopExhausted: eventLoopExhausted,
        rejectionTracked: rejectionTracked,
        reportUnhandledRejection: reportUnhandledRejection,
        resolveModule: resolveModule,
        moduleSource: moduleSource,
        // The error a specifier that resolves to nothing fails with, which is the one a
        // `require` of it throws: the host raises it from its own resolver, so an `import`
        // that resolves to nothing fails the way a `require` of the same specifier does.
        moduleNotFound: moduleNotFound,
        requireId: requireId,
        // The three computations the `crypto` core module is served by, and the two reads
        // the `fs` one answers from the read-only mounts.
        digest: function (algorithm, data) {
            var digested = hostDigest(algorithm, data);
            if (digested === null || digested === undefined) {
                throw new Error('Digest method not supported');
            }
            return digested;
        },
        hmac: function (algorithm, key, data) {
            var keyed = hostHmac(algorithm, key, data);
            if (keyed === null || keyed === undefined) {
                throw new Error('Digest method not supported');
            }
            return keyed;
        },
        randomHex: function (count) {
            var drawn = hostRandomHex(count);
            if (drawn === null || drawn === undefined) {
                throw new RangeError('The value of "size" is out of range.');
            }
            return drawn;
        },
        fileSource: function (path) {
            var id = typeof path === 'string' ? idOfPath(path) : null;
            return id === null ? null : sourceOf(id);
        },
        filePaths: function () {
            var held = hostModulePaths();
            var out = [];
            for (var index = 0; index < held.length; index += 1) {
                out.push(filenameOf(held[index]));
            }
            return out;
        },
        // The ONE call the AWS SDK this runtime supplies makes: the host decides which
        // services are reachable, signs the request and dispatches it.
        awsCall: function (service, operation, region, body) {
            return hostAwsCall(service, operation, region, body);
        },
        // The offset of a named time zone at an instant, which `Intl` and the local reading
        // of `Date` stand on.
        zoneOffsetMs: function (zone, time) {
            var offset = hostZoneOffsetMs(zone, time);
            return offset === undefined ? null : offset;
        },
        // The two text codings bytes are converted with, computed by the host's own
        // implementations of them.
        encode: function (coding, bytes) {
            var encoded = hostEncode(coding, bytes);
            if (encoded === null || encoded === undefined) {
                throw new TypeError('Unknown encoding: ' + coding);
            }
            return encoded;
        },
        decode: function (coding, text) {
            var decoded = hostDecode(coding, text);
            if (decoded === null || decoded === undefined) {
                throw new TypeError('Unknown encoding: ' + coding);
            }
            return decoded;
        }
    });
})();
"##;
