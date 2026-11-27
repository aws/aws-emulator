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
//! functions [`super::install_host`] installs — the invocation log, a package entry's source,
//! a core module's source, the pure computations `crypto` and `zlib` are served by, the one
//! reading of a host name the execution environment's own resolver answers, the one AWS SDK
//! call and the one request a handler sends itself — and this source takes every one of them
//! off the global object once it holds them. The deadline, the step ceiling and the memory
//! ceiling are the engine's and the host's ([`super::Bounds`]), so no rebinding or deletion of
//! a name here can lift one.
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
    var hostRecord = globalThis.__hostRecord;
    var hostModuleSource = globalThis.__hostModuleSource;
    var hostModulePaths = globalThis.__hostModulePaths;
    var hostCoreSource = globalThis.__hostCoreSource;
    var hostEphemeralBytes = globalThis.__hostEphemeralBytes;
    var hostDigest = globalThis.__hostDigest;
    var hostHmac = globalThis.__hostHmac;
    var hostCipher = globalThis.__hostCipher;
    var hostCiphers = globalThis.__hostCiphers;
    var hostHkdf = globalThis.__hostHkdf;
    var hostPbkdf2 = globalThis.__hostPbkdf2;
    var hostDeflate = globalThis.__hostDeflate;
    var hostInflate = globalThis.__hostInflate;
    var hostCrc32 = globalThis.__hostCrc32;
    var hostResolve = globalThis.__hostResolve;
    var hostRandomHex = globalThis.__hostRandomHex;
    var hostAwsCall = globalThis.__hostAwsCall;
    var hostHttpSend = globalThis.__hostHttpSend;
    var hostCompileScript = globalThis.__hostCompileScript;
    var hostZoneOffsetMs = globalThis.__hostZoneOffsetMs;
    var hostEncode = globalThis.__hostEncode;
    var hostDecode = globalThis.__hostDecode;
    var hostBeginInvocation = globalThis.__hostBeginInvocation;
    delete globalThis.__hostLog;
    delete globalThis.__hostRecord;
    delete globalThis.__hostModuleSource;
    delete globalThis.__hostModulePaths;
    delete globalThis.__hostCoreSource;
    delete globalThis.__hostEphemeralBytes;
    delete globalThis.__hostDigest;
    delete globalThis.__hostHmac;
    delete globalThis.__hostCipher;
    delete globalThis.__hostCiphers;
    delete globalThis.__hostHkdf;
    delete globalThis.__hostPbkdf2;
    delete globalThis.__hostDeflate;
    delete globalThis.__hostInflate;
    delete globalThis.__hostCrc32;
    delete globalThis.__hostResolve;
    delete globalThis.__hostRandomHex;
    delete globalThis.__hostAwsCall;
    delete globalThis.__hostHttpSend;
    delete globalThis.__hostCompileScript;
    delete globalThis.__hostZoneOffsetMs;
    delete globalThis.__hostEncode;
    delete globalThis.__hostDecode;
    delete globalThis.__hostBeginInvocation;

    // Dynamic import is the one way a module of the package is loaded as an ES module. It
    // is compiled here, once, so a run never depends on it being available at any later
    // point — and through the same seam a module of the package is compiled by, under the
    // name node holds its own ES loader at ([`internalUnitOf`]), so a frame taken inside it
    // names a module of the runtime rather than an anonymous compilation of the host's.
    var dynamicImport = null;
    try {
        dynamicImport = hostCompileScript(
            internalUnitOf('modules/esm/loader'),
            'export default function (specifier) {\n    return import(specifier);\n};\n'
        );
        if (typeof dynamicImport !== 'function') {
            dynamicImport = null;
        }
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
    // The ENGINE's own `stack` accessor, kept as this runtime found it on `Error.prototype`
    // before wrapping it ([`installErrorStack`]): it answers the frames of an error, which is
    // what the wrapper reports under the head line a node runtime opens a stack with.
    var engineStack = null;

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
    //
    // A path carries ONE separator per step, and a mount's root is a directory in its own
    // right: the id of a root carries no tail at all, and the path of that directory is the
    // mount itself — `/var/task`, never `/var/task/`. That is what a module sitting at the
    // root of the deployment package reads as its own `__dirname`, and what a path joined
    // onto it has to be built from.
    function filenameOf(id) {
        if (id.indexOf(CORE_PREFIX) === 0) {
            return 'node:' + id.slice(CORE_PREFIX.length);
        }
        var root = rootOf(id);
        var mount = TASK_ROOT;
        if (root === LAYER_PREFIX) {
            mount = LAYER_ROOT;
        } else if (root === RUNTIME_PREFIX) {
            mount = RUNTIME_ROOT;
        }
        var tail = tailOf(id);
        return tail === '' ? mount : mount + '/' + tail;
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

    // ---------------------------------------------------------------- package exports

    // The conditions this runtime resolves a package's `exports` under, one set per KIND of
    // load: a `require` takes the conditions node activates for one and an `import` takes its
    // own, so a dependency that publishes a CommonJS entry point beside an ES one is resolved
    // to the entry point for the way it is being loaded. `node` is the platform this is, and
    // `default` is the unconditional fallback every map may end with.
    //
    // Which of a map's own keys applies is decided by the MAP's order — "the first condition
    // that matches" — so these are sets a key has to be in, not an order.
    var REQUIRE_CONDITIONS = ['node', 'node-addons', 'require', 'default'];
    var IMPORT_CONDITIONS = ['node', 'node-addons', 'import', 'default'];

    // The target a conditional value states, with `star` substituted for the `*` of a pattern
    // key (and `null` where the key carried none). A string is the target itself, an array is
    // the first of its alternatives that states one, and an object is a map of conditions.
    // `null` for a value stating no target this resolution may take.
    function exportTarget(held, star, conditions, hops) {
        if (typeof held === 'string') {
            return star === null ? held : held.split('*').join(star);
        }
        if (hops >= MAX_MANIFEST_HOPS || held === null || typeof held !== 'object') {
            return null;
        }
        var picked = null;
        var index = 0;
        if (Array.isArray(held)) {
            for (index = 0; index < held.length; index += 1) {
                picked = exportTarget(held[index], star, conditions, hops + 1);
                if (picked !== null) {
                    return picked;
                }
            }
            return null;
        }
        var keys = Object.keys(held);
        for (index = 0; index < keys.length; index += 1) {
            if (conditions.indexOf(keys[index]) >= 0) {
                picked = exportTarget(held[keys[index]], star, conditions, hops + 1);
                if (picked !== null) {
                    return picked;
                }
            }
        }
        return null;
    }

    // Whether `held` is a SUBPATH MAP — every key of it names a subpath — as against the
    // conditions of the single export `.` is, whose keys are condition names.
    function subpathMap(held) {
        var keys = Object.keys(held);
        if (keys.length === 0) {
            return false;
        }
        for (var index = 0; index < keys.length; index += 1) {
            if (keys[index].charAt(0) !== '.') {
                return false;
            }
        }
        return true;
    }

    // The target a package's `exports` states for `subpath` — `.` for the package itself and
    // `./x` for a subpath of it — or `null` for a subpath that package does not export.
    //
    // An exact key wins; past it a key carrying `*` is a PATTERN, and the one whose literal
    // prefix is longest is the one that applies, with what the `*` stood for substituted into
    // its target. A value that is not a subpath map states the one export `.` is, which is the
    // shorthand form `"exports": "./index.js"` and the conditions-only form both take.
    function exportedTarget(exported, subpath, conditions) {
        if (typeof exported !== 'object' || exported === null || Array.isArray(exported)
            || !subpathMap(exported)) {
            return subpath === '.' ? exportTarget(exported, null, conditions, 0) : null;
        }
        if (Object.prototype.hasOwnProperty.call(exported, subpath)) {
            var exact = exportTarget(exported[subpath], null, conditions, 0);
            if (exact !== null) {
                return exact;
            }
        }
        var keys = Object.keys(exported);
        var best = null;
        var longest = -1;
        for (var index = 0; index < keys.length; index += 1) {
            var key = keys[index];
            var star = key.indexOf('*');
            if (star < 0) {
                continue;
            }
            var head = key.slice(0, star);
            var tail = key.slice(star + 1);
            if (subpath.indexOf(head) !== 0 || subpath.length < head.length + tail.length) {
                continue;
            }
            if (tail !== '' && subpath.slice(subpath.length - tail.length) !== tail) {
                continue;
            }
            if (head.length > longest) {
                longest = head.length;
                best = exportTarget(
                    exported[key],
                    subpath.slice(head.length, subpath.length - tail.length),
                    conditions,
                    0
                );
            }
        }
        return best;
    }

    // The package a bare specifier names, and the subpath of that package it asks for: the
    // first segment is the package's name, or the first two where the name is a scoped one.
    function packageSpecifier(request) {
        var segments = request.split('/');
        var count = request.charAt(0) === '@' ? 2 : 1;
        if (segments.length < count || segments[0] === '') {
            return null;
        }
        var rest = segments.slice(count).join('/');
        return {
            name: segments.slice(0, count).join('/'),
            subpath: rest === '' ? '.' : './' + rest
        };
    }

    // The entry the package at `root` exports for `subpath`, for a package whose manifest
    // declares `exports`, and `null` for one that does not declare it at all — which is what
    // tells this resolution apart from the legacy one.
    //
    // A package that DOES declare it is resolved through that map and through nothing else:
    // "the exports field … encapsulates the package, preventing consumers from reaching inside
    // it", so a file the map does not state is not an entry of that package even though the
    // package carries it. The suffix and directory forms are still tried on the target the map
    // states, because a map may name a directory's index or a path without its extension.
    function exportedCandidate(root, subpath, conditions) {
        var manifest = sourceOf(root + '/package.json');
        if (manifest === null) {
            return null;
        }
        var declared = null;
        try {
            declared = JSON.parse(manifest).exports;
        } catch (error) {
            declared = null;
        }
        if (declared === null || declared === undefined) {
            return null;
        }
        var target = exportedTarget(declared, subpath, conditions);
        var joined = target === null ? null : packagePath(root, target);
        if (joined === null) {
            // The package states no such export, so it has none: the resolution stops here
            // rather than falling through to the file the map left unstated, and WHY it
            // stopped is held for the caller to report ([`unexported`]).
            unexported = {
                subpath: subpath,
                manifest: filenameOf(root + '/package.json')
            };
            return { entry: null };
        }
        return { entry: sourceOf(joined) !== null ? joined : fileCandidate(joined) };
    }

    function packageCandidate(directory, request, conditions) {
        var named = packageSpecifier(request);
        var current = directory;
        for (;;) {
            var root = named === null
                ? null
                : packagePath(current, 'node_modules/' + named.name);
            var exported = root === null
                ? null
                : exportedCandidate(root, named.subpath, conditions);
            if (exported !== null) {
                return exported.entry;
            }
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
    function resolveRequest(directory, request, conditions) {
        var active = conditions === undefined ? REQUIRE_CONDITIONS : conditions;
        // Whatever the last resolution refused is no longer this one's: a specifier that
        // resolves to nothing for any other reason is the module-not-found it is.
        unexported = null;
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
        return packageCandidate(directory, request, active);
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

    // What the last resolution refused because the package's own `exports` map does not state
    // the subpath asked for, held until the caller reports it ([`packageNotExported`]).
    //
    // The package WAS found, and it states that it does not export that path, which is a
    // different failure from a module that is nowhere: node reports it under its own code, and
    // a program branching on that code is what the difference is for. It is set by
    // [`exportedCandidate`] and cleared at the head of every resolution.
    var unexported = null;

    // That refusal, taken off the holder: it answers ONE failure, so a later resolution that
    // refused nothing reports a module that could not be found.
    function takeUnexported() {
        var held = unexported;
        unexported = null;
        return held;
    }

    function moduleNotFound(request) {
        var refused = takeUnexported();
        if (refused !== null) {
            return packageNotExported(refused);
        }
        var error = new Error("Cannot find module '" + request + "'");
        error.code = 'MODULE_NOT_FOUND';
        return error;
    }

    // The failure a specifier the package does not export fails with: the message node writes
    // for it, naming the subpath that was asked for and the manifest that does not state it.
    function packageNotExported(held) {
        var error = new Error("Package subpath '" + held.subpath
            + '\' is not defined by "exports" in ' + held.manifest);
        error.code = 'ERR_PACKAGE_PATH_NOT_EXPORTED';
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
        var refused = takeUnexported();
        if (refused !== null) {
            return packageNotExported(refused);
        }
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
    // module is compiled the same way, under the internal name [`internalUnitOf`] states.
    function compileModule(name, wrapped) {
        var compiled = hostCompileScript(name, wrapped);
        if (typeof compiled !== 'function') {
            throw new Error("Cannot compile module '" + name + "'");
        }
        return compiled;
    }

    // The unit name the runtime's own implementation of the core module `name` is compiled
    // under: node holds the implementation of a module a handler imports by its `node:`
    // name under its INTERNAL namespace, and that is the name a frame taken inside one
    // carries. It is not a name any specifier resolves to — module resolution answers
    // package files and the `node:` core names alone ([`resolveModule`]) — so it names a
    // source of this execution environment without being a module guest code can import.
    function internalUnitOf(name) {
        return 'node:internal/' + name;
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
        // The exports object the module SCOPE is given, kept so the names its exports were
        // assigned under can be stated once its body has run ([`nameModuleExports`]).
        var scopeExports = module.exports;
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
                // A CORE module is compiled as a unit of its own too, under the INTERNAL
                // name the runtime holds its implementation of that module by: a frame
                // taken inside it names that module, the way a frame inside one of node's
                // own internals does, rather than the pseudo-file the engine's function
                // constructor would give an anonymous compilation. The name cannot be the
                // `node:` name the module is imported by, because that one is already the
                // unit of its own ES wrapper ([`requiredWrapper`]).
                var compiled = compileModule(
                    core ? internalUnitOf(id.slice(CORE_PREFIX.length)) : filename,
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
                nameModuleExports(module, scopeExports, core);
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

    // State the name a function was ASSIGNED under, for a function carrying none of its own.
    //
    // A stack frame names the function it is in by the name that function carries, and a
    // function written without one carries the name the language INFERS for it. The engine a
    // managed node runtime embeds infers one for a function expression assigned to a PROPERTY
    // as well, naming it by the assignment path it was written at; the engine embedded here
    // infers one only where the language states it (a declaration, a variable initialiser, a
    // member of an object literal), and a function it inferred nothing for reads as an
    // anonymous one in every frame of it. A function that carries a name already — one the
    // language stated, or one inferred at the position it was written at — keeps it, because
    // that is the name the frame carries on a managed runtime too.
    function nameAssigned(value, assigned) {
        if (typeof value !== 'function') {
            return;
        }
        try {
            if (value.name !== '') {
                return;
            }
            Object.defineProperty(value, 'name', {
                value: assigned,
                writable: false,
                enumerable: false,
                configurable: true
            });
        } catch (error) {
            // A function whose name cannot be read or stated keeps the one it has: naming a
            // frame is no reason for a module to fail to load.
        }
    }

    // The names a CommonJS module's exports were assigned under, stated on the functions
    // among them once the module's body has run ([`nameAssigned`]): the members of the
    // exports object the module scope was given are `exports.<member>`, a module whose
    // exports it REPLACED with a function is `module.exports`, and the members of an object
    // it replaced them with are `module.exports.<member>`.
    //
    // Only a member held as a VALUE is named: reading an accessor would run the module's own
    // code to decide a name. A module the RUNTIME supplies its own implementation of is left
    // alone: node compiles its internals as its own and a frame inside one names that
    // internal module, not an export of the package that imported it.
    function nameModuleExports(module, scopeExports, core) {
        if (core) {
            return;
        }
        var exported = module.exports;
        if (exported === null || exported === undefined) {
            return;
        }
        var replaced = exported !== scopeExports;
        nameAssigned(exported, 'module.exports');
        if (typeof exported !== 'object' && typeof exported !== 'function') {
            return;
        }
        var prefix = replaced ? 'module.exports.' : 'exports.';
        var keys = [];
        try {
            keys = Object.keys(exported);
        } catch (error) {
            // Exports whose members cannot be enumerated are named by nothing.
            return;
        }
        for (var index = 0; index < keys.length; index += 1) {
            var held = null;
            try {
                held = Object.getOwnPropertyDescriptor(exported, keys[index]);
            } catch (error) {
                held = null;
            }
            if (held !== null && held !== undefined && 'value' in held) {
                nameAssigned(held.value, prefix + keys[index]);
            }
        }
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
        var resolved = resolveRequest(directory, name, IMPORT_CONDITIONS);
        return resolved === null ? null : engineName(resolved);
    }

    // ---------------------------------------------------------------- the console

    var util = requireId('core:util');

    // The value of `AWS_LAMBDA_LOG_FORMAT` whose records are STRUCTURED, which is the member
    // `LoggingConfig.LogFormat` carries: "the format in which Lambda sends your function's
    // application and system logs to CloudWatch. Select between plain text and structured
    // JSON".
    var JSON_LOG_FORMAT = 'JSON';

    // The frame a managed node runtime delivers a console line in, under the TEXT log
    // format: the instant it was written, the invocation it belongs to, the LEVEL of the
    // console method that wrote it, and the formatted text — four tab-separated fields, in
    // that order, which is what lets a client correlate a line to an invocation and filter
    // by level. It is the same frame the runtime's own `Invoke Error` record carries
    // ([`logInvokeError`]), so one runtime writes one kind of line.
    //
    // Under the STRUCTURED format the same record is ONE JSON object instead — the level,
    // the message, the invocation and the instant as named members — because a function whose
    // `LogFormat` is JSON has its application records delivered as records rather than as
    // text ([`jsonRecords`]).
    //
    // A line written while NO invocation is bound — at module load, before a request
    // reaches the handler — belongs to no invocation, so it is delivered as the text it is
    // rather than attributed to one.
    //
    // Every one of these is a record the RUNTIME framed, so it is handed to the platform as
    // one record and reaches CloudWatch Logs as one event however many line breaks the
    // formatted text carries — a multi-line `console.log`, the stack of an error. That is
    // what tells it apart from a write the handler made straight to `process.stdout`, which
    // the platform frames by its lines.
    function writeLine(level, args, line) {
        var text = line === undefined
            ? util.format.apply(null, Array.prototype.slice.call(args))
            : String(line);
        if (state === null || state.requestId === null || state.requestId === undefined) {
            hostRecord(text + '\n');
            return;
        }
        if (!admittedLevel(level)) {
            return;
        }
        if (jsonRecords()) {
            hostRecord(jsonRecord(level, Array.prototype.slice.call(args), text) + '\n');
            return;
        }
        hostRecord(new Date().toISOString() + '\t' + state.requestId + '\t' + level + '\t'
            + text + '\n');
    }

    // How much DETAIL each level names, in the order `ApplicationLogLevel` documents —
    // "TRACE is the highest level and FATAL is the lowest" — which is the ladder the level
    // the invocation's environment states is read on.
    //
    // Source: the model's `ApplicationLogLevel` enum (TRACE | DEBUG | INFO | WARN | ERROR |
    // FATAL).
    var LOG_LEVEL_DETAIL = {
        TRACE: 0,
        DEBUG: 1,
        INFO: 2,
        WARN: 3,
        ERROR: 4,
        FATAL: 5
    };

    // Whether a record at `level` is one this runtime writes at all: "Lambda only sends
    // application logs at the selected level of detail and lower", and the managed runtimes
    // apply that where the record is written. A function filtered at no level writes every
    // record, and so does a record at a level the ladder does not place.
    function admittedLevel(level) {
        var configured = state === null ? null : state.logLevel;
        if (configured === null || configured === undefined || configured === '') {
            return true;
        }
        var threshold = LOG_LEVEL_DETAIL[String(configured).toUpperCase()];
        var detail = LOG_LEVEL_DETAIL[level];
        if (threshold === undefined || detail === undefined) {
            return true;
        }
        return detail >= threshold;
    }

    // ONE application record under the STRUCTURED format: the instant it was written, its
    // level, the invocation it belongs to and the MESSAGE.
    //
    // A single argument stays the value it is — an object logged under this format is a JSON
    // object a client parses rather than the runtime's inspect text of it, and an Error is
    // the error document it describes ([`errorMessage`]) — while several arguments are the
    // formatted line they make, exactly as the text format states it. An Error among several
    // arguments names its own class, message and stack on the record beside that line, which
    // is what makes a logged failure readable as a record.
    function jsonRecord(level, args, text) {
        var record = {
            timestamp: new Date().toISOString(),
            level: level,
            requestId: state.requestId
        };
        if (args.length === 1 && args[0] !== undefined) {
            record.message = errorDocument(args[0]);
        } else {
            record.message = text;
            for (var index = 0; index < args.length; index += 1) {
                if (args[index] instanceof Error) {
                    var reported = raised(args[index]);
                    record.errorType = reported.errorType;
                    record.errorMessage = reported.errorMessage;
                    record.stackTrace = reported.trace;
                    break;
                }
            }
        }
        return recordText(record, text);
    }

    // `value` as a record's message states it: the error DOCUMENT for an error — its class,
    // its message and its stack, which is how a structured record carries one — and the value
    // itself for anything else.
    function errorDocument(value) {
        if (!(value instanceof Error)) {
            return value;
        }
        var reported = raised(value);
        return {
            errorType: reported.errorType,
            errorMessage: reported.errorMessage,
            stackTrace: reported.trace
        };
    }

    // `record` as the text of one record. A message the serializer cannot carry — a value
    // holding a cycle — leaves the record carrying the formatted line instead, so a log call
    // never fails the invocation that made it.
    function recordText(record, text) {
        try {
            return JSON.stringify(record);
        } catch (error) {
            record.message = text;
            return JSON.stringify(record);
        }
    }

    // Whether this invocation's records are the structured ones, which the invocation's own
    // environment states as the function's configured `LogFormat`.
    function jsonRecords() {
        return state !== null && state.logFormat === JSON_LOG_FORMAT;
    }

    // The console of this runtime is the `console` core module's own surface — its whole
    // documented method set, which is also what a `Console` a handler builds over its own
    // streams carries — writing through the record channel above, which is what makes
    // `require('console') === console` hold here as it does on node.
    globalThis.console = requireId(CORE_PREFIX + 'console').__recordWith(writeLine);

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
        installErrorStack();
        installErrorText();
        // The performance clock, and the worker types a program holds a channel's two ends
        // by: taken off the core module that implements each, so a handler that imports the
        // module and one that reads the global hold the same object.
        globalThis.performance = requireId(CORE_PREFIX + 'perf_hooks').performance;
        var threads = requireId(CORE_PREFIX + 'worker_threads');
        globalThis.MessageChannel = threads.MessageChannel;
        globalThis.MessagePort = threads.MessagePort;
        globalThis.Worker = threads.Worker;
        // The rest of the WHATWG surface ([`super::web_globals`]), installed with this
        // runtime's own `require` so that those globals and the core modules behind them are
        // one and the same. The installer is taken off the global object here, before a line
        // of the package is loaded, exactly as the host functions were.
        if (typeof globalThis.__installWebGlobals === 'function') {
            var installWebGlobals = globalThis.__installWebGlobals;
            delete globalThis.__installWebGlobals;
            installWebGlobals(requireFrom('', [RUNTIME_ENTRY]));
        }
        // "Error.stackTraceLimit ... The default value is 10": the number of frames an
        // error's own stack carries, which `NODE_OPTIONS` can set.
        Error.stackTraceLimit = 10;
    }

    // ---------------------------------------------------------------- error stacks

    // The frames the ENGINE took for `error`, as its own text: one `    at <name> (<unit>)`
    // line per frame, and nothing else — the engine's stack carries no head line.
    //
    // Read through the accessor `Error.prototype` carried before this runtime wrapped it, so
    // it answers the frames even for an error whose `stack` a handler has since assigned.
    function engineFrames(error) {
        if (engineStack === null) {
            return undefined;
        }
        try {
            return engineStack.call(error);
        } catch (unavailable) {
            return undefined;
        }
    }

    // The line a stack OPENS with: the error's own string representation — `<name>: <message>`,
    // the name alone for an error carrying no message, and the message alone for one carrying
    // no name.
    //
    // The message is the one the RUNTIME words for that error class ([`decorated`]), not the
    // raw text the engine built: this line is what a handler that CAUGHT the error reads off
    // its `stack` and writes to its log, and what the error document of one that did not
    // catch it reports, so both read one wording of one failure.
    function errorHead(error) {
        var name = error.name === undefined || error.name === null ? 'Error' : String(error.name);
        var message = error.message === undefined || error.message === null
            ? ''
            : decorated(String(error.message), error);
        if (name === '') {
            return message;
        }
        return message === '' ? name : name + ': ' + message;
    }

    // The property a PROPERTY ACCESS failure named, for the error the engine raises when the
    // value it was read from or written to is `undefined` or `null`; `null` for every other
    // error. The wording the runtime reports for the class is [`decorated`]'s; this reads the
    // engine's own, which is where the property the access named is stated.
    function accessedProperty(error) {
        if (!(error instanceof TypeError)) {
            return null;
        }
        var access = propertyAccess(error.message);
        return access === null ? null : access.property;
    }

    // The unit, line and column one frame line names, or `null` for a line that names no
    // position at all (a frame inside the engine's own code, which carries none).
    function framePosition(line) {
        var tail = /:(\d+):(\d+)(\)?)$/.exec(line);
        if (tail === null) {
            return null;
        }
        var head = line.slice(0, line.length - tail[0].length);
        var open = head.lastIndexOf('(');
        return {
            unit: open < 0 ? head.replace(/^\s*at\s*/, '') : head.slice(open + 1),
            line: Number(tail[1]),
            column: Number(tail[2]),
            // The line up to the position it ends with, and the parenthesis that closes it:
            // what [`withColumn`] states another column between.
            head: head,
            closed: tail[3]
        };
    }

    // That same frame line, naming `column` instead of the one it carries.
    function withColumn(position, column) {
        return position.head + ':' + position.line + ':' + column + position.closed;
    }

    // Line `at` of the unit a frame names, as the file holding it was written, or `null` for
    // a unit this function holds no source for — the runtime's own entry module among them.
    function unitLine(unit, at) {
        var path = unit.indexOf(FILE_URL_PREFIX) === 0
            ? unit.slice(FILE_URL_PREFIX.length)
            : unit;
        var id = idOfPath(path);
        var source = id === null ? null : sourceOf(id);
        if (source === null) {
            return null;
        }
        var lines = source.split('\n');
        return at >= 1 && at <= lines.length ? lines[at - 1] : null;
    }

    // The column `property` is written at on `text`, at or after `column`, as the position a
    // frame names it by — `0` where the line does not state that access, which is every line
    // naming the property by an expression rather than writing it.
    function propertyColumn(text, column, property) {
        var from = column < 1 ? 0 : column - 1;
        var written = '.' + property;
        for (;;) {
            var at = text.indexOf(written, from);
            if (at < 0) {
                return 0;
            }
            // The written name ends the token it is: a longer name beginning with it is
            // another property, and the access that failed is not in it.
            var after = text.charAt(at + written.length);
            if (after === '' || !/[A-Za-z0-9_$]/.test(after)) {
                return at + 2;
            }
            from = at + 1;
        }
    }

    // The frames of a PROPERTY ACCESS failure, its innermost frame naming the position the
    // property is written at.
    //
    // A managed runtime names that frame by the property the access named, which is what
    // tells a client WHICH access on the line failed; the engine embedded here names the
    // position the expression the property is read from begins at instead. The property is in
    // the failure's own message and the module's own source says where on the line it is
    // written, so the frame the access was made in — the innermost one, the frames under it
    // being named by their own calls — is named by that position.
    function atAccessedProperty(frames, property) {
        var lines = frames.split('\n');
        var innermost = lines.length === 0 ? null : framePosition(lines[0]);
        if (innermost === null) {
            return frames;
        }
        var text = unitLine(innermost.unit, innermost.line);
        if (text === null) {
            return frames;
        }
        var column = propertyColumn(text, innermost.column, property);
        if (column === 0) {
            return frames;
        }
        lines[0] = withColumn(innermost, column);
        return lines.join('\n');
    }

    // `stack` as a node runtime reports it: the error's own head line ([`errorHead`]), then one
    // line per frame the engine took.
    //
    // The engine answers the FRAMES alone, so this runtime wraps its accessor — on
    // `Error.prototype`, which is where the engine keeps it and where no error carries one of
    // its own, so EVERY error reads this way whether the engine raised it or the handler
    // constructed it. That head line is what tells code reading a failure by its `stack` WHAT
    // failed: a handler that reports an error's stack — which is what a custom resource
    // provider sends CloudFormation as its failure — otherwise reports frames with no class and
    // no message in front of them.
    //
    // Assigning `stack` still replaces it: the assignment puts an own property on the error, as
    // it does on a managed runtime, so what a handler put there is what reads back.
    function installErrorStack() {
        var held = Object.getOwnPropertyDescriptor(Error.prototype, 'stack');
        if (held === undefined || typeof held.get !== 'function') {
            return;
        }
        engineStack = held.get;
        Object.defineProperty(Error.prototype, 'stack', {
            configurable: true,
            get: function () {
                var taken = engineFrames(this);
                // `Error.prepareStackTrace`: where one is installed, what IT answers is the
                // stack — head line and all — so nothing is put in front of it.
                if (typeof Error.prepareStackTrace === 'function') {
                    return taken;
                }
                var frames = typeof taken === 'string' ? taken : '';
                while (frames !== '' && frames.charAt(frames.length - 1) === '\n') {
                    frames = frames.slice(0, frames.length - 1);
                }
                // A property access that failed is named by the property it named
                // ([`atAccessedProperty`]).
                var property = frames === '' ? null : accessedProperty(this);
                if (property !== null) {
                    frames = atAccessedProperty(frames, property);
                }
                var head = errorHead(this);
                return frames === '' ? head : head + '\n' + frames;
            },
            set: function (value) {
                Object.defineProperty(this, 'stack', {
                    value: value,
                    writable: true,
                    enumerable: false,
                    configurable: true
                });
            }
        });
    }

    // An error AS TEXT — `String(error)`, `` `${error}` `` and `error.toString()`, the three
    // spellings a handler writes one to its log by: the head line a stack opens with
    // ([`errorHead`]), which is the `<name>: <message>` an `Error.prototype.toString` answers
    // and carries the wording this runtime states for the class.
    function installErrorText() {
        Object.defineProperty(Error.prototype, 'toString', {
            configurable: true,
            writable: true,
            value: function () {
                return errorHead(this);
            }
        });
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
    // line, stated here from the class and the message this runtime reports, so the wording
    // it reports and the wording the trace opens with are one. Anything else reports the TYPE
    // of the value and the value as text, with no trace at all, because a value that is not an
    // error carries no stack to report.
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
        var trace = traceLines(type + ': ' + decorated(message, error));
        var frames = tracedFrames(error);
        if (frames !== '') {
            var lines = frames.split('\n');
            for (var index = 0; index < lines.length; index += 1) {
                if (lines[index] !== '') {
                    trace.push(lines[index]);
                }
            }
        }
        return {
            outcome: 'raised',
            errorType: type,
            errorMessage: decorated(message, error),
            trace: trace
        };
    }

    // The part of an error's `stack` that follows the head line it opens with
    // ([`errorHead`]) — which, for an error whose stack a handler ASSIGNED, is the text that
    // handler put there, since an assigned stack opens with no head line of this runtime's.
    function tracedFrames(error) {
        var stack = typeof error.stack === 'string' ? error.stack : '';
        var head = errorHead(error);
        return head !== '' && stack.indexOf(head) === 0 ? stack.slice(head.length) : stack;
    }

    // How many entries of a reported trace its HEAD line occupies: one per line of the
    // "<ErrorType>: <message>" line a trace opens with ([`traceLines`]).
    function headLineCount(reported) {
        return traceLines(reported.errorType + ': ' + reported.errorMessage).length;
    }

    // The message the RUNTIME reports for an error class whose own wording it states rather
    // than the engine's. Five of them:
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
    // * a CALL of a value that is not callable names the EXPRESSION the call named in front
    //   of the failure — "<expression> is not a function" ([`calledExpression`]).
    function decorated(message, error) {
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
        var access = propertyAccess(message);
        if (access !== null) {
            return access.read
                ? 'Cannot read properties of ' + access.value
                    + " (reading '" + access.property + "')"
                : 'Cannot set properties of ' + access.value
                    + " (setting '" + access.property + "')";
        }
        if (message === NOT_A_FUNCTION) {
            var called = calledExpression(error);
            return called === null ? message : called + ' is not a function';
        }
        return message;
    }

    // The property ACCESS the engine's own wording states: which direction the access was in,
    // the property it named, and the value — `undefined` or `null` — it was made on. `null`
    // for a message that is not that failure's.
    //
    // The ONE place this runtime reads that wording. Both what it reports for the class
    // ([`decorated`]) and the position it names the access at ([`atAccessedProperty`]) are
    // stated from what this answers, so one reading of the engine's message serves both.
    function propertyAccess(message) {
        var access = /^cannot (read|set) property '(.*)' of (undefined|null)$/.exec(
            typeof message === 'string' ? message : ''
        );
        if (access === null) {
            return null;
        }
        return { read: access[1] === 'read', property: access[2], value: access[3] };
    }

    // What the engine words a CALL of a value that is not callable as: the failure alone,
    // naming nothing of the call it was made at.
    var NOT_A_FUNCTION = 'not a function';

    // The characters an identifier, a property name or a number is written with.
    var WORD = /[A-Za-z0-9_$]/;

    // The EXPRESSION a call that failed named, as the module's own source writes it — what a
    // managed runtime reports in front of that failure, and `null` where this runtime cannot
    // read it off the source.
    //
    // The engine states the failure alone, and its innermost frame names the position the
    // call was made at: the property a MEMBER call names, and the start of the expression a
    // call of any other form names. From that position the expression is read off the line
    // the frame names — the chain the call is the end of, with any argument list INSIDE it
    // elided, which is how a managed runtime renders one.
    //
    // Reading it is best-effort by nature: a frame of a module this function holds no source
    // for (the runtime's own), and a position the line does not state a call at, are answered
    // with nothing at all, so the engine's own wording stands rather than a guessed one.
    function calledExpression(error) {
        if (!(error instanceof TypeError)) {
            return null;
        }
        try {
            var frames = engineFrames(error);
            var lines = typeof frames === 'string' ? frames.split('\n') : [];
            var innermost = lines.length === 0 ? null : framePosition(lines[0]);
            if (innermost === null || innermost.column < 1) {
                return null;
            }
            var text = unitLine(innermost.unit, innermost.line);
            return text === null ? null : calleeWritten(text, innermost.column - 1);
        } catch (unreadable) {
            // The wording of a failure is not itself a failure: an expression that cannot be
            // read leaves the engine's own message as the one reported.
            return null;
        }
    }

    // The callee of the call written at `at` on `text`: from where the expression begins to
    // the argument list the call hands it.
    function calleeWritten(text, at) {
        var before = lastWordlessBefore(text, at);
        var connector = before < 0 ? '' : text.charAt(before);
        var list = null;
        if (connector === '.') {
            // A member call: the position is the property, and the list follows it.
            list = listAfter(text, wordEnd(text, at));
        } else if (connector === '[') {
            // A member call through a computed key: the list follows the key's brackets.
            var closed = groupEnd(text, before);
            list = closed < 0 ? null : listAfter(text, closed + 1);
        } else {
            // Any other call: the position is the start of the callee, whose own chain the
            // list ends.
            list = lastListOfChain(text, at);
        }
        if (list === null) {
            return null;
        }
        var written = text.slice(calleeStart(text, at), list).trim();
        return written === '' ? null : elidedLists(written);
    }

    // The index of the last character before `at` that is not a space, or `-1` for a position
    // with nothing but spaces in front of it.
    function lastWordlessBefore(text, at) {
        var index = at;
        while (index > 0 && /\s/.test(text.charAt(index - 1))) {
            index -= 1;
        }
        return index - 1;
    }

    // The index just past the identifier, property name or number written at `from`.
    function wordEnd(text, from) {
        var index = from;
        while (index < text.length && WORD.test(text.charAt(index))) {
            index += 1;
        }
        return index;
    }

    // The index of the argument list opening at or after `from`, or `null` where what follows
    // is not a call at all.
    function listAfter(text, from) {
        var index = from;
        while (index < text.length && /\s/.test(text.charAt(index))) {
            index += 1;
        }
        return text.charAt(index) === '(' ? index : null;
    }

    // The index of the bracket closing the group that opens at `open`, or `-1` for a group
    // the line does not close. Nested groups and the text of a string literal are stepped
    // over, so the bracket answered is the one that closes this group.
    function groupEnd(text, open) {
        var depth = 0;
        var index = open;
        while (index < text.length) {
            var ch = text.charAt(index);
            if (ch === "'" || ch === '"' || ch === '`') {
                index = stringEnd(text, index);
                continue;
            }
            if (ch === '(' || ch === '[' || ch === '{') {
                depth += 1;
            } else if (ch === ')' || ch === ']' || ch === '}') {
                depth -= 1;
                if (depth === 0) {
                    return index;
                }
            }
            index += 1;
        }
        return -1;
    }

    // The index just past the string literal that opens at `open`.
    function stringEnd(text, open) {
        var quote = text.charAt(open);
        var index = open + 1;
        while (index < text.length) {
            var ch = text.charAt(index);
            if (ch === '\\') {
                index += 2;
                continue;
            }
            index += 1;
            if (ch === quote) {
                return index;
            }
        }
        return index;
    }

    // The LAST argument list of the member-and-call chain that starts at `at`: the list of
    // the call the chain ends in, which is the call that was made of the expression in front
    // of it. `null` for a chain holding no call at all.
    function lastListOfChain(text, at) {
        var index = at;
        var list = null;
        while (index < text.length) {
            var ch = text.charAt(index);
            if (WORD.test(ch) || ch === '.' || ch === '?') {
                index += 1;
                continue;
            }
            if (ch === '(' || ch === '[') {
                var closed = groupEnd(text, index);
                if (closed < 0) {
                    return ch === '(' ? index : list;
                }
                if (ch === '(') {
                    list = index;
                }
                index = closed + 1;
                continue;
            }
            return list;
        }
        return list;
    }

    // Where the expression a call was made of BEGINS, walking back from the position `at` the
    // frame names: over each member step and the operand it was taken on, so the whole chain
    // in front of the call is named and nothing in front of the chain is.
    function calleeStart(text, at) {
        var start = at;
        for (;;) {
            var before = lastWordlessBefore(text, start);
            if (before < 0) {
                return start;
            }
            var step = before;
            var ch = text.charAt(before);
            if (ch === '.') {
                // The same step, written as an optional chain.
                var optional = lastWordlessBefore(text, before);
                step = optional >= 0 && text.charAt(optional) === '?' ? optional : before;
            } else if (ch !== '[') {
                return start;
            }
            var operand = operandStart(text, step);
            if (operand === null) {
                return start;
            }
            start = operand;
        }
    }

    // Where the operand that ends just before `before` begins — an identifier or number, a
    // bracketed group, or a string literal — or `null` for anything else, which is not part of
    // the chain the call was made on.
    function operandStart(text, before) {
        var end = lastWordlessBefore(text, before);
        if (end < 0) {
            return null;
        }
        var ch = text.charAt(end);
        if (ch === ')' || ch === ']') {
            var open = groupStart(text, end);
            return open < 0 ? null : open;
        }
        if (ch === "'" || ch === '"' || ch === '`') {
            var quoted = stringStart(text, end);
            return quoted < 0 ? null : quoted;
        }
        if (WORD.test(ch)) {
            var index = end;
            while (index > 0 && WORD.test(text.charAt(index - 1))) {
                index -= 1;
            }
            return index;
        }
        return null;
    }

    // The index of the bracket that opens the group closing at `close`, or `-1` for a group
    // the line does not open.
    function groupStart(text, close) {
        var depth = 0;
        var index = close;
        while (index >= 0) {
            var ch = text.charAt(index);
            if (ch === ')' || ch === ']' || ch === '}') {
                depth += 1;
            } else if (ch === '(' || ch === '[' || ch === '{') {
                depth -= 1;
                if (depth === 0) {
                    return index;
                }
            }
            index -= 1;
        }
        return -1;
    }

    // The index of the quote that opens the string literal closing at `close`, or `-1` for a
    // literal the line does not open.
    function stringStart(text, close) {
        var quote = text.charAt(close);
        var index = close - 1;
        while (index >= 0) {
            if (text.charAt(index) === quote && text.charAt(index - 1) !== '\\') {
                return index;
            }
            index -= 1;
        }
        return -1;
    }

    // The chain `written` with every argument list in it elided: a managed runtime names what
    // was called rather than what each call inside it was handed, so a list is reported as the
    // one it is and not as its arguments.
    function elidedLists(written) {
        var out = '';
        var index = 0;
        while (index < written.length) {
            var ch = written.charAt(index);
            // A `(` that opens a CALL's list follows the expression called; one that opens a
            // grouped expression follows nothing, and is written as the source has it.
            var after = index === 0 ? '' : written.charAt(lastWordlessBefore(written, index));
            if (ch === '(' && (WORD.test(after) || after === ')' || after === ']')) {
                var closed = groupEnd(written, index);
                if (closed < 0) {
                    return out + written.slice(index);
                }
                out = out + '(...)';
                index = closed + 1;
                continue;
            }
            out = out + ch;
            index += 1;
        }
        return out;
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
    //
    // Under the STRUCTURED format it is ONE JSON object like every other record of such a
    // function, carrying the error's class, message and stack as named members.
    function logInvokeError(reported) {
        if (state === null || state.loggedError) {
            return;
        }
        state.loggedError = true;
        if (!admittedLevel('ERROR')) {
            return;
        }
        if (jsonRecords()) {
            hostRecord(JSON.stringify({
                timestamp: new Date().toISOString(),
                level: 'ERROR',
                requestId: state.requestId,
                message: {
                    errorType: reported.errorType,
                    errorMessage: reported.errorMessage,
                    stackTrace: reported.trace
                }
            }) + '\n');
            return;
        }
        hostRecord(new Date().toISOString() + '\t' + state.requestId + '\tERROR\tInvoke Error \t'
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
            identity: input.cognitoIdentity === null || input.cognitoIdentity === undefined
                ? undefined
                : input.cognitoIdentity,
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

    // Call the handler the invocation resolved, with the event and the context held for it
    // ([`invoke`]).
    //
    // The call is made from a JOB of the runtime's own, so this is the only frame of the
    // runtime underneath the deployment package's own frames in a stack taken inside the
    // handler: a managed runtime AWAITS the handler's answer, and what scheduled the call it
    // awaits is no longer on the stack by the time the handler runs. The arguments are read
    // off the invocation rather than passed in for that reason — a function that passed them
    // on would be a frame of the runtime on that stack too.
    function callHandler() {
        var entry = state.invocation.entry;
        var event = state.invocation.event;
        var context = state.invocation.context;
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
            state.invocation = {
                entry: entry,
                event: event,
                context: makeContext(input)
            };
            // The handler runs in a job of the runtime's own, which is what the only frame
            // of the runtime on a stack taken inside it comes from ([`callHandler`]).
            return Promise.resolve().then(callHandler);
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
                    'SyntaxError: ' + decorated(error.message, error)
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
            // The FORM this invocation's records take, which its own environment states.
            logFormat: input.environment.AWS_LAMBDA_LOG_FORMAT,
            // The LEVEL its application records are filtered at, which its own environment
            // states as the function's `ApplicationLogLevel`: a record more detailed than
            // this is one the runtime does not write at all ([`admittedLevel`]).
            logLevel: input.environment.AWS_LAMBDA_LOG_LEVEL,
            esModuleHint: null,
            loggedError: false,
            rejections: [],
            // What the handler is called with, once the invocation has resolved it
            // ([`callHandler`]).
            invocation: null
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
        // A FRESH instance of a module of this function's own mounts, which is what a
        // WORKER's entry point is: a worker shares no module cache with the thread that
        // started it, so the module is compiled and run again — with its own `module` and
        // `exports`, its own `require` resolving from its own directory, and its own
        // `__filename`/`__dirname`. `source` stands in for the file's own text, which is what
        // an entry point passed as source rather than as a path is
        // ([`core_modules`]'s `worker_threads`, `new Worker(code, { eval: true })`).
        //
        // It is no new reach: the path is resolved by the same [`idOfPath`] every other
        // lookup goes through, so only an entry of this function's own mounts is read, and
        // the `require` the instance is given is the one every module of the package gets.
        freshModule: function (filename, source) {
            var id = typeof filename === 'string' ? idOfPath(filename) : null;
            if (id === null) {
                return null;
            }
            var text = typeof source === 'string' ? source : sourceOf(id);
            if (text === null) {
                return null;
            }
            var named = filenameOf(id);
            var directory = directoryOf(id);
            return function () {
                var module = {
                    id: named,
                    filename: named,
                    exports: {},
                    loaded: false,
                    children: [],
                    paths: searchPaths(directory)
                };
                var compiled = compileModule(
                    named,
                    'export default function (exports, require, module, __filename, '
                        + '__dirname) {' + text + '\n};\n'
                );
                compiled.call(
                    module.exports,
                    module.exports,
                    requireFrom(directory, [named, RUNTIME_ENTRY]),
                    module,
                    named,
                    filenameOf(directory)
                );
                module.loaded = true;
                return module.exports;
            };
        },
        // The three computations the `crypto` core module is served by, the symmetric
        // transform beside them, and the two reads the `fs` one answers from the read-only
        // mounts.
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
        // The symmetric transform of one message: the parts go over as one list and the
        // answer's first word says whether it was computed ([`super::cipher`]).
        cipher: function (parts, decrypting) {
            var answered = hostCipher(parts, decrypting === true);
            return answered === null || answered === undefined ? ['unknown'] : answered;
        },
        // The ciphers the host computes, which is the one list the `crypto` module reports
        // and resolves a name against.
        ciphers: function () {
            var named = hostCiphers();
            return named === null || named === undefined ? [] : named;
        },
        // The key a digest derives from the parts a derivation is given, which is the one
        // the construction itself states ([`super::digest`]).
        hkdf: function (parts, length) {
            var derived = hostHkdf(parts, length);
            if (derived === null || derived === undefined) {
                throw new RangeError('Invalid key length');
            }
            return derived;
        },
        // The key the iterated construction derives from the parts a password-based
        // derivation is given ([`super::digest::pbkdf2`]).
        pbkdf2: function (parts, iterations, length) {
            var derived = hostPbkdf2(parts, iterations, length);
            if (derived === null || derived === undefined) {
                throw new RangeError('Invalid key length');
            }
            return derived;
        },
        // The two directions of the DEFLATE stream the `zlib` core module computes
        // ([`super::compress`]): the wrapper names the stream, and a wrapper this runtime
        // does not compute — or a call past the host's own ceiling — is refused rather than
        // answered with bytes of another stream.
        deflate: function (format, level, data) {
            var compressed = hostDeflate(format, level, data);
            if (compressed === null || compressed === undefined) {
                throw new Error('unsupported compression');
            }
            return compressed;
        },
        inflate: function (format, data) {
            var inflated = hostInflate(format, data);
            if (inflated === null || inflated === undefined) {
                throw new Error('incorrect data check');
            }
            return inflated;
        },
        // The checksum that same module computes over bytes ([`super::compress::crc32`]).
        crc32: function (data, seed) {
            var checksum = hostCrc32(data, seed);
            if (checksum === null || checksum === undefined) {
                throw new RangeError('Invalid data length');
            }
            return checksum;
        },
        // The addresses a host name resolves to inside this execution environment, which is
        // the ONE reading of a name the whole runtime shares
        // ([`super::super::guest_resolver`]): a name no zone this emulator holds names an
        // address under resolves to none.
        resolveAddresses: function (host) {
            var resolved = hostResolve(typeof host === 'string' ? host : '');
            return resolved === null || resolved === undefined ? [] : resolved;
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
        // How large the `/tmp` of this execution environment is, which the filesystem module
        // writes into and reports: the function's own `EphemeralStorage.Size`, as the host
        // states it.
        ephemeralBytes: function () {
            return hostEphemeralBytes();
        },
        // The ONE call the AWS SDK this runtime supplies makes: the host decides which
        // services are reachable, signs the request and dispatches it. The credentials the
        // CLIENT was built with travel with the call, so a client built with a session the
        // handler was vended is signed as that session; one built with none is signed as the
        // identity the invocation was given. The ENDPOINT it was built with travels for the
        // same reason: that is where the SDK sends such a client's calls.
        awsCall: function (service, operation, region, body, credentials, endpoint) {
            return hostAwsCall(
                service,
                operation,
                region,
                body,
                typeof credentials === 'string' ? credentials : '',
                typeof endpoint === 'string' ? endpoint : ''
            );
        },
        // The ONE request the `http` and `https` core modules send: the host decides which
        // addresses are reachable, signs the request and dispatches it.
        httpSend: function (method, url, headers, body) {
            return hostHttpSend(method, url, headers, body);
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
