// SPDX-License-Identifier: CC0-1.0

//! The GUEST a MicroVM runs: the program held in the image version's code artifact.
//!
//! `CodeArtifact$uri` is "The URI of the code artifact in Amazon S3", and the object at
//! that address is what a MicroVM's guest IS: the client's own code, which a MicroVM is
//! run to serve traffic with. The build resolves that object against the emulator's own
//! object store ([`super::code_artifact`]); this module reads what it fetched and keeps
//! the modules the guest is made of, so the endpoint can RUN them
//! ([`super::guest_runtime`]) instead of answering in the guest's name.
//!
//! # What is read
//!
//! * a `.zip` artifact — the archive form a build context is uploaded as — has its
//!   entries read ENTIRELY IN MEMORY: no entry is written to a host path and nothing
//!   outside this process is opened for it;
//! * an artifact that is not an archive is the guest's single module, which is the other
//!   form a one-file program is staged as.
//!
//! Only entries the embedded engine can COMPILE are kept: JavaScript source
//! ([`JAVASCRIPT_SUFFIXES`]) and the JSON a program reads beside it. A compiled
//! artefact, a native library or a base-image layer in the artifact is NOT loaded —
//! nothing here loads a host library — so a guest that needs one holds no program here
//! and nothing is reported as having run.
//!
//! # The entry module
//!
//! Which module STARTS the guest is the artifact's own declaration, read in the order a
//! container image declares it:
//!
//! 1. the JavaScript file the image's `Dockerfile` names in its `CMD` or `ENTRYPOINT` —
//!    the command the container runs;
//! 2. the `main` of a `package.json` beside it, then `index.js` — Node's own module
//!    resolution ("If the main field is absent … Node.js will attempt to load
//!    `index.js`");
//! 3. the single JavaScript module, where the artifact holds exactly one.
//!
//! An artifact declaring none of those has no entry, so it holds no guest program: what
//! starts the guest is not guessed from the content of its code.
//!
//! # Bounds
//!
//! Every bound is on what THIS PROCESS holds rather than on what the archive declares,
//! because the declared sizes are the caller's: [`MAX_ARTIFACT_BYTES`] of fetched object,
//! [`MAX_ENTRIES`] entries read, [`MAX_PROGRAM_BYTES`] of inflated source across the
//! archive and [`MAX_MODULE_BYTES`] of any one module, each counted as the bytes are
//! produced.

use std::collections::BTreeMap;
use std::io::Read;

/// How many bytes of a fetched code artifact are read into this process.
pub const MAX_ARTIFACT_BYTES: u64 = 16 * 1024 * 1024;

/// How many entries of one archive are read.
pub const MAX_ENTRIES: usize = 2_048;

/// How many INFLATED bytes of source one artifact may yield.
pub const MAX_PROGRAM_BYTES: usize = 8 * 1024 * 1024;

/// How many INFLATED bytes ONE module may hold.
pub const MAX_MODULE_BYTES: usize = 2 * 1024 * 1024;

/// The suffixes of an entry the embedded JavaScript engine can load: the module forms
/// Node resolves (`.js`, and the explicit `.mjs`/`.cjs`) and the JSON a program reads
/// beside its code.
const JAVASCRIPT_SUFFIXES: [&str; 4] = [".js", ".mjs", ".cjs", ".json"];

/// The module Node loads when a package declares no `main`: "If the main field is absent
/// … Node.js will attempt to load `index.js`".
const DEFAULT_ENTRY: &str = "index.js";

/// The file a container image's build context declares its command in.
const CONTAINER_SPEC: &str = "Dockerfile";

/// The package manifest a Node program declares its entry module in.
const PACKAGE_MANIFEST: &str = "package.json";

/// One module of a guest program: the path it is stored under inside the artifact, and
/// its source text.
#[derive(Debug, Clone)]
pub struct GuestModule {
    /// The artifact-relative path, with `\` normalised to `/` and any leading `./`
    /// removed, so a `require` of a relative path resolves the name a real runtime
    /// resolves.
    pub path: String,
    /// The module's source text.
    pub source: String,
}

/// The program a MicroVM's guest runs: every module the artifact holds, and the one that
/// starts it.
#[derive(Debug, Clone)]
pub struct GuestProgram {
    /// The modules, keyed by their artifact-relative path.
    pub modules: BTreeMap<String, String>,
    /// The path of the module that starts the guest.
    pub entry: String,
}

/// The guest program `bytes` hold, or `None` when they hold none this engine can run.
///
/// `key` is the object key the artifact was fetched from, which names the single module
/// of an artifact that is not an archive.
#[must_use]
pub fn read(bytes: &[u8], key: &str) -> Option<GuestProgram> {
    let entries = match archive_entries(bytes) {
        Some(entries) => entries,
        // Not an archive this emulator reads: the object is the guest's own single
        // module, under the name it was stored as.
        None => vec![GuestModule {
            path: module_path(key),
            source: String::from_utf8(bytes.to_vec()).ok()?,
        }],
    };
    program(entries)
}

/// The program `entries` make up: the modules the engine can load, and the entry the
/// artifact declares.
fn program(entries: Vec<GuestModule>) -> Option<GuestProgram> {
    let declared = declared_entry(&entries);
    let modules: BTreeMap<String, String> = entries
        .into_iter()
        .filter(|entry| is_loadable(&entry.path))
        .map(|entry| (entry.path, entry.source))
        .collect();
    let entry = resolve_entry(&modules, declared.as_deref())?;
    Some(GuestProgram { modules, entry })
}

/// Whether the engine can load the module stored under `path`.
fn is_loadable(path: &str) -> bool {
    JAVASCRIPT_SUFFIXES
        .iter()
        .any(|suffix| path.to_ascii_lowercase().ends_with(suffix))
}

/// The entry the artifact DECLARES: its container command first, then its package
/// manifest's `main`.
fn declared_entry(entries: &[GuestModule]) -> Option<String> {
    let text = |name: &str| {
        entries
            .iter()
            .find(|entry| entry.path == name)
            .map(|entry| entry.source.as_str())
    };
    if let Some(command) = text(CONTAINER_SPEC).and_then(container_command_module) {
        return Some(command);
    }
    text(PACKAGE_MANIFEST).and_then(package_main)
}

/// The JavaScript module a `Dockerfile`'s command names, from the LAST `CMD` or
/// `ENTRYPOINT` instruction it carries (a later one replaces an earlier).
///
/// Both forms an instruction is written in are read — the JSON array (`CMD ["node",
/// "server.js"]`) and the shell form (`CMD node server.js`) — and what is taken from it
/// is the first word naming a JavaScript module. An instruction naming none yields
/// nothing.
fn container_command_module(dockerfile: &str) -> Option<String> {
    dockerfile
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let (instruction, arguments) = trimmed.split_once(char::is_whitespace)?;
            let instruction = instruction.to_ascii_uppercase();
            if instruction != "CMD" && instruction != "ENTRYPOINT" {
                return None;
            }
            arguments
                .split(|byte: char| byte.is_ascii_whitespace() || "[],\"'".contains(byte))
                .map(module_path)
                .find(|word| is_loadable(word) && !word.ends_with(".json"))
        })
        .next_back()
}

/// The `main` a `package.json` declares, as the path it names.
fn package_main(manifest: &str) -> Option<String> {
    let parsed: ::serde_json::Value = ::serde_json::from_str(manifest).ok()?;
    let main = parsed.get("main")?.as_str()?;
    Some(module_path(main))
}

/// The module that starts the guest: the declared entry when the artifact holds it, then
/// Node's default entry, then the single module of a one-module artifact.
fn resolve_entry(modules: &BTreeMap<String, String>, declared: Option<&str>) -> Option<String> {
    if let Some(declared) = declared.filter(|path| modules.contains_key(*path)) {
        return Some(declared.to_string());
    }
    if modules.contains_key(DEFAULT_ENTRY) {
        return Some(DEFAULT_ENTRY.to_string());
    }
    let mut scripts = modules.keys().filter(|path| !path.ends_with(".json"));
    let only = scripts.next()?;
    scripts.next().is_none().then(|| only.clone())
}

/// The entries of `bytes` read as a `.zip` archive, under this module's bounds — or
/// `None` when the bytes are not an archive this emulator reads.
fn archive_entries(bytes: &[u8]) -> Option<Vec<GuestModule>> {
    let mut archive = ::zip::ZipArchive::new(::std::io::Cursor::new(bytes)).ok()?;
    let mut entries: Vec<GuestModule> = Vec::new();
    let mut left = MAX_PROGRAM_BYTES;
    for index in 0..archive.len().min(MAX_ENTRIES) {
        let Ok(mut entry) = archive.by_index(index) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let Ok(name) = entry.name() else {
            continue;
        };
        let path = module_path(name.as_ref());
        // The read is capped by what is LEFT of the program budget rather than by the
        // size the archive declares, so an entry that inflates past it stops there.
        let cap = left.min(MAX_MODULE_BYTES);
        let mut held = Vec::new();
        if entry
            .by_ref()
            .take(u64::try_from(cap).unwrap_or_default().saturating_add(1))
            .read_to_end(&mut held)
            .is_err()
            || held.len() > cap
        {
            continue;
        }
        left = left.saturating_sub(held.len());
        if let Ok(source) = String::from_utf8(held) {
            entries.push(GuestModule { path, source });
        }
    }
    Some(entries)
}

/// One stored path as a module path: `\` normalised to `/`, any leading `./` removed.
fn module_path(path: &str) -> String {
    let normalised = path.replace('\\', "/");
    normalised
        .strip_prefix("./")
        .unwrap_or(&normalised)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the entry-module table: the case, the artifact's entries as
    /// `(path, source)` pairs, and the entry that must start the guest.
    type EntryRow<'a> = (&'a str, Vec<(&'a str, &'a str)>, Option<&'a str>);

    /// RULE: the module that starts the guest is the one the artifact DECLARES — the
    /// container command first, then the package manifest's `main`, then Node's default
    /// entry, then the single module of a one-module artifact — and an artifact
    /// declaring none holds no program.
    #[test]
    fn the_entry_module_is_the_one_the_artifact_declares() {
        // (case, the entries, the entry that must start the guest)
        let rows: Vec<EntryRow> = vec![
            (
                "a container command in array form",
                vec![
                    ("Dockerfile", "FROM node:24-alpine\nCMD [\"node\", \"server.js\"]\n"),
                    ("server.js", "// the guest"),
                    ("index.js", "// not the command"),
                ],
                Some("server.js"),
            ),
            (
                "a container command in shell form",
                vec![
                    ("Dockerfile", "FROM node:24-alpine\nENTRYPOINT node app/main.js\n"),
                    ("app/main.js", "// the guest"),
                    ("index.js", "// not the command"),
                ],
                Some("app/main.js"),
            ),
            (
                "the last command instruction wins",
                vec![
                    ("Dockerfile", "CMD [\"node\", \"first.js\"]\nCMD [\"node\", \"second.js\"]\n"),
                    ("first.js", "// replaced"),
                    ("second.js", "// the guest"),
                ],
                Some("second.js"),
            ),
            (
                "a package manifest's main",
                vec![
                    ("package.json", "{\"main\": \"./lib/boot.js\"}"),
                    ("lib/boot.js", "// the guest"),
                    ("index.js", "// not the main"),
                ],
                Some("lib/boot.js"),
            ),
            (
                "no declaration at all",
                vec![("index.js", "// the guest"), ("other.js", "// a module")],
                Some("index.js"),
            ),
            (
                "a single module",
                vec![("guest.js", "// the guest")],
                Some("guest.js"),
            ),
            (
                "several modules and no declaration",
                vec![("a.js", "// one"), ("b.js", "// another")],
                None,
            ),
            (
                "nothing the engine can load",
                vec![("rootfs.img", "\u{0}binary"), ("Dockerfile", "FROM scratch")],
                None,
            ),
            (
                "a declared entry the artifact does not hold",
                vec![
                    ("Dockerfile", "CMD [\"node\", \"absent.js\"]"),
                    ("index.js", "// the guest"),
                ],
                Some("index.js"),
            ),
        ];

        for (case, entries, expected) in rows {
            let entries: Vec<GuestModule> = entries
                .into_iter()
                .map(|(path, source)| GuestModule {
                    path: path.to_string(),
                    source: source.to_string(),
                })
                .collect();
            let actual = program(entries).map(|program| program.entry);
            let expected = expected.map(::std::string::ToString::to_string);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the entry {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: an artifact that is not an archive this emulator reads is the guest's own
    /// single module, under the name the object key gave it.
    #[test]
    fn a_bare_object_is_the_guests_single_module() {
        let program = read(b"// the guest", "team/build/guest.js")
            .expect("a JavaScript object is a one-module guest program");
        assert_eq!(
            program.entry, "team/build/guest.js",
            "expected the object's own key as the entry, got {:?}",
            program.entry
        );
        assert_eq!(
            program.modules.get("team/build/guest.js").map(String::as_str),
            Some("// the guest"),
            "expected the object's bytes as the module source, got {:?}",
            program.modules
        );
    }
}
