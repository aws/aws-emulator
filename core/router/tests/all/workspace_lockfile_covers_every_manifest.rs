// SPDX-License-Identifier: CC0-1.0

//! The committed `Cargo.lock` covers every dependency the workspace's manifests declare.
//!
//! The release build resolves with `--locked`, which forbids cargo from writing
//! `Cargo.lock`: a dependency some manifest declares that the lock does not carry at an
//! accepted version stops the build before a single crate compiles, with
//! `cannot update the lock file ... because --locked was passed`. This suite reads the
//! committed manifests and the committed lock and pairs them, so a dependency added
//! without refreshing the lock fails here — in the workspace's own tests — instead of in
//! a platform build nobody can rerun locally.
//!
//! It reads files only; it resolves nothing, spawns nothing and reaches no network.
//!
//! # Why it reads the two files itself
//!
//! The suite adds no dependency to the router, so it reads the manifests and the lock —
//! both generated — line by line rather than through a TOML or semver crate, joining a
//! declaration that runs over several lines back into one. It is conservative by
//! construction: a requirement form it does not recognise counts as satisfied, so it can
//! only fail on a dependency the lock genuinely omits, never on manifest syntax it has not
//! met. `an_unlocked_dependency_is_reported` pins that it does fail on one, and
//! `no_manifest_declares_a_dependency_in_a_form_the_suite_cannot_read` pins that the
//! declaration forms it does not read — a `[dependencies.<name>]` sub-table and a dotted
//! key — are absent from the tree rather than silently skipped.
//!
//! Those readers are `pub(crate)`: `sql_engine_single_locked_copy` and
//! `workspace_lockfile_records_each_member_dependency` pair the same two committed files
//! for a different invariant, so the lock and the manifests are read here once rather than
//! by a second reader of the same formats. This module owns the ONE reader of each format:
//! `lock_entries` reads the lock's `[[package]]` records whole, and every other view of it —
//! `locked_packages`, which is crate name to locked versions — is derived from that one.
//!
//! The same holds for the rule those suites state over the two files. A suite that pins ONE
//! locked copy of a component at a bounded release — `sql_engine_single_locked_copy` for the
//! SQL engine the data planes link, `python_guest_arithmetic_single_locked_copy` for the
//! Python guest's arithmetic — brings its crates, its `ReleaseBound` and the release it
//! refuses, and reads them through `locked_copy_reports` and `pinned_requirement_reports`
//! here, so a third such suite copies no checker and the existing two cannot drift apart.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// A dependency as a manifest declares it: `name = "<req>"`, or an inline table with any
/// of `version`, `package` (the crate the lock knows, when the key is an alias),
/// `workspace = true` (the requirement lives in the root `[workspace.dependencies]`) and
/// `optional = true` (a feature has to turn it on before it is resolved at all).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Declared {
    pub(crate) package: Option<String>,
    pub(crate) req: Option<String>,
    pub(crate) from_workspace: bool,
    pub(crate) optional: bool,
}

/// The crate names the lock carries, each with the versions locked for it.
pub(crate) type Locked = BTreeMap<String, Vec<String>>;

/// The workspace root: this crate's manifest sits at `<root>/core/router`.
pub(crate) fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the router crate sits two directories below the workspace root")
        .to_path_buf()
}

pub(crate) fn unquote(value: &str) -> &str {
    value.trim().trim_matches('"')
}

/// One `[[package]]` of the lock: the crate it locks, where it comes from, the checksum of
/// the archive to fetch (a registry crate only), and the crates it depends on.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct LockEntry {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) source: Option<String>,
    pub(crate) checksum: Option<String>,
    pub(crate) dependencies: BTreeSet<String>,
}

impl LockEntry {
    /// Whether this package is fetched from a registry, and so needs a checksum. A member of
    /// this workspace and a path dependency have no source at all.
    pub(crate) fn is_from_registry(&self) -> bool {
        self.source
            .as_deref()
            .is_some_and(|source| source.starts_with("registry+"))
    }
}

/// Every `[[package]]` the lock carries, whole and in file order.
///
/// An edge is written either as the crate's name alone or, where the lock holds more than one
/// version of it, as the name and the version (`"sha2 0.10.9"`); the name is what a manifest
/// declares, so that is what is kept.
pub(crate) fn lock_entries(lock: &str) -> Vec<LockEntry> {
    let mut entries: Vec<LockEntry> = Vec::new();
    let mut current: Option<LockEntry> = None;
    let mut inside_dependencies = false;
    for line in lock.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside_dependencies = false;
            entries.extend(current.take());
            if line == "[[package]]" {
                current = Some(LockEntry::default());
            }
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        if inside_dependencies {
            if line.starts_with(']') {
                inside_dependencies = false;
            } else if let Some(name) = unquote(line.trim_end_matches(',')).split_whitespace().next()
            {
                entry.dependencies.insert(name.to_string());
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("name = ") {
            entry.name = unquote(value).to_string();
        } else if let Some(value) = line.strip_prefix("version = ") {
            entry.version = unquote(value).to_string();
        } else if let Some(value) = line.strip_prefix("source = ") {
            entry.source = Some(unquote(value).to_string());
        } else if let Some(value) = line.strip_prefix("checksum = ") {
            entry.checksum = Some(unquote(value).to_string());
        } else if line.starts_with("dependencies = [") {
            inside_dependencies = true;
        }
    }
    entries.extend(current);
    entries
}

/// Every `[[package]]` in the lock, as crate name -> the versions locked for it, read off the
/// records `lock_entries` returns.
pub(crate) fn locked_packages(lock: &str) -> Locked {
    let mut locked: Locked = BTreeMap::new();
    for entry in lock_entries(lock) {
        locked.entry(entry.name).or_default().push(entry.version);
    }
    locked
}

/// The directories listed in the root manifest's `members` array.
pub(crate) fn workspace_members(root_manifest: &str) -> Vec<String> {
    let mut members = Vec::new();
    let mut inside = false;
    for line in root_manifest.lines() {
        let line = line.trim();
        if !inside {
            inside = line.starts_with("members") && line.ends_with('[');
            continue;
        }
        if line.starts_with(']') {
            break;
        }
        let entry = unquote(line.trim_end_matches(','));
        if !entry.is_empty() && !entry.starts_with('#') {
            members.push(entry.to_string());
        }
    }
    members
}

/// The `name` of the `[package]` a manifest defines.
pub(crate) fn package_name(manifest: &str) -> Option<String> {
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[package]";
        } else if inside {
            if let Some(value) = line.strip_prefix("name = ") {
                return Some(unquote(value).to_string());
            }
        }
    }
    None
}

fn parse_declared(value: &str) -> Declared {
    let Some(table) = value.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) else {
        return Declared {
            req: Some(unquote(value).to_string()),
            ..Declared::default()
        };
    };
    let mut declared = Declared::default();
    for field in table.split(',') {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        match key.trim() {
            "package" => declared.package = Some(unquote(value).to_string()),
            "version" => declared.req = Some(unquote(value).to_string()),
            "workspace" => declared.from_workspace = unquote(value) == "true",
            "optional" => declared.optional = unquote(value) == "true",
            _ => {}
        }
    }
    declared
}

/// Whether a line closes every brace and bracket it opens, counting only those outside a
/// quoted string: a brace inside a value (a `description` with one in it) opens nothing.
fn is_balanced(line: &str) -> bool {
    let mut depth = 0i32;
    let mut quoted = false;
    for character in line.chars() {
        match character {
            '"' => quoted = !quoted,
            '{' | '[' if !quoted => depth += 1,
            '}' | ']' if !quoted => depth -= 1,
            _ => {}
        }
    }
    depth <= 0
}

/// A manifest's lines, with a declaration spread over several of them joined back into one:
/// cargo lets an inline table's feature array run over lines, and a declaration read as its
/// first line alone would hide the version requirement the rest of it carries.
fn logical_lines(manifest: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut open: Option<String> = None;
    for line in manifest.lines() {
        let line = line.trim();
        let joined = match open.take() {
            Some(mut pending) => {
                pending.push(' ');
                pending.push_str(line);
                pending
            }
            None if line.starts_with('#') => {
                lines.push(line.to_string());
                continue;
            }
            None => line.to_string(),
        };
        if is_balanced(&joined) {
            lines.push(joined);
        } else {
            open = Some(joined);
        }
    }
    lines.extend(open);
    lines
}

/// Every dependency a manifest declares in the tables `wanted` accepts, as the table's
/// header, the name the manifest uses and the declaration. A name may be declared once per
/// table — `[dependencies]` and `[dev-dependencies]` can require different versions of one
/// crate — so these are a sequence, not a map keyed by name.
pub(crate) fn dependencies_in(
    manifest: &str,
    wanted: fn(&str) -> bool,
) -> Vec<(String, String, Declared)> {
    let mut declared = Vec::new();
    let mut table = String::new();
    let mut inside = false;
    for line in logical_lines(manifest) {
        let line = line.trim();
        if line.starts_with('[') {
            inside = wanted(line);
            if inside {
                table = line.to_string();
            }
            continue;
        }
        if !inside || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        // `version.workspace = true` and its siblings are package fields, not dependencies.
        if key.is_empty() || key.contains('.') {
            continue;
        }
        declared.push((table.clone(), key.to_string(), parse_declared(value.trim())));
    }
    declared
}

/// The root `[workspace.dependencies]` table a member's `workspace = true` resolves through,
/// keyed by the name a member declares. It is one table, so no name appears twice.
pub(crate) fn workspace_dependency_table(root_manifest: &str) -> BTreeMap<String, Declared> {
    dependencies_in(root_manifest, is_workspace_dependency_table)
        .into_iter()
        .map(|(_table, name, declared)| (name, declared))
        .collect()
}

pub(crate) fn is_member_dependency_table(section: &str) -> bool {
    matches!(
        section,
        "[dependencies]" | "[dev-dependencies]" | "[build-dependencies]"
    ) || (section.starts_with("[target.") && section.ends_with("dependencies]"))
}

fn is_workspace_dependency_table(section: &str) -> bool {
    section == "[workspace.dependencies]"
}

/// One report per dependency `manifest` declares in a form `dependencies_in` reads as
/// nothing, and so would leave unchecked: a `[dependencies.<name>]` sub-table, whose header
/// `is_member_dependency_table` does not accept, and a dotted key inside a dependency table
/// (`serde.workspace = true`), which that reader skips because the same spelling in
/// `[package]` is a package field rather than a dependency. Both are failed here rather than
/// passed over in silence.
fn unreadable_dependency_forms(manifest_path: &str, manifest: &str) -> Vec<String> {
    let mut reports = Vec::new();
    let mut inside = false;
    for (number, line) in manifest.lines().enumerate() {
        let line = line.trim();
        if line.starts_with('[') {
            // `[workspace.dependencies]` and `[target.'…'.dependencies]` put their dot
            // BEFORE the word; a sub-table is the only header with one after it.
            if line.contains("dependencies.") {
                reports.push(format!(
                    "case {manifest_path:?} line {}: expected the dependency in a \
                     `[…dependencies]` table, got the sub-table header {line:?}",
                    number + 1
                ));
            }
            inside = is_member_dependency_table(line) || is_workspace_dependency_table(line);
            continue;
        }
        if !inside || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line
            .split_once('=')
            .is_some_and(|(key, _)| key.trim().contains('.'))
        {
            reports.push(format!(
                "case {manifest_path:?} line {}: expected the whole declaration on the \
                 dependency's own key, got the dotted key {line:?}",
                number + 1
            ));
        }
    }
    reports
}

/// `major.minor.patch` and how many of the three the text actually spelled out.
pub(crate) fn parse_version(value: &str) -> Option<([u64; 3], usize)> {
    let mut parsed = [0u64; 3];
    let mut spelled = 0usize;
    for part in value.trim().split('.') {
        if spelled == 3 {
            return None;
        }
        parsed[spelled] = part.parse().ok()?;
        spelled += 1;
    }
    (spelled > 0).then_some((parsed, spelled))
}

/// Whether `version` falls inside the requirement `req`, for the two forms this workspace's
/// manifests use: cargo's default caret requirement (bare or `^`-prefixed), which admits
/// versions from the one spelled out up to the next increment of its leftmost non-zero
/// component, and the `=` pin, which admits only the components it spells out. Any other
/// form — a comparison, a tilde, a wildcard, a union, a prerelease — is reported accepted:
/// this suite pins lock COVERAGE, and cargo stays the authority on the requirement forms it
/// does not read here.
pub(crate) fn accepts(req: &str, version: &str) -> bool {
    let Some((actual, _)) = parse_version(version) else {
        return true;
    };
    let req = req.trim();
    if let Some(pinned) = req.strip_prefix('=') {
        let Some((wanted, spelled)) = parse_version(pinned) else {
            return true;
        };
        return wanted[..spelled] == actual[..spelled];
    }
    let Some((lower, spelled)) = parse_version(req.strip_prefix('^').unwrap_or(req)) else {
        return true;
    };
    let bump = (0..spelled)
        .find(|&slot| lower[slot] != 0)
        .unwrap_or(spelled - 1);
    let mut upper = lower;
    upper[bump] += 1;
    for slot in upper.iter_mut().skip(bump + 1) {
        *slot = 0;
    }
    actual >= lower && actual < upper
}

/// The bound a suite that pins ONE locked copy of a component holds that copy's release to:
/// at or past a floor, or below a ceiling.
///
/// Both exist because the two reasons to pin point opposite ways: a release that first carries
/// a fix is a floor, and the first release this project's toolchain cannot compile is a
/// ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReleaseBound {
    AtOrPast(&'static str),
    Below(&'static str),
}

impl ReleaseBound {
    /// The release the bound is written at.
    pub(crate) fn release(self) -> &'static str {
        match self {
            Self::AtOrPast(release) | Self::Below(release) => release,
        }
    }

    /// How the bound reads in a report.
    fn wording(self) -> &'static str {
        match self {
            Self::AtOrPast(_) => "at or past",
            Self::Below(_) => "below",
        }
    }

    /// Whether `version` satisfies the bound, compared component by component as
    /// `major.minor.patch`.
    ///
    /// `None` where either side is not written as numbers — which a caller FAILS on rather than
    /// passes over, so an unreadable release can never be read as one the bound admits.
    pub(crate) fn admits(self, version: &str) -> Option<bool> {
        let (bound, _) = parse_version(self.release())?;
        let (version, _) = parse_version(version)?;
        Some(match self {
            Self::AtOrPast(_) => version >= bound,
            Self::Below(_) => version < bound,
        })
    }
}

/// One report per way the lock fails `bound` for the crate `name`, under the label `case`: the
/// lock carries no copy of it, it carries more than one, or a copy it carries lies outside the
/// bound (or is written in a form no release comparison can read).
///
/// This is the ONE reading of "how many copies, at which release" in this target: a suite that
/// pins a single locked copy of a component — the SQL engine the data planes link, the Python
/// guest's arithmetic — states its crate and its bound and reads them through here, rather than
/// through a second copy of the rule that could drift from this one.
pub(crate) fn locked_copy_reports(
    locked: &Locked,
    case: &str,
    name: &str,
    bound: ReleaseBound,
) -> Vec<String> {
    let Some(versions) = locked.get(name) else {
        return vec![format!(
            "case {case:?}: expected Cargo.lock to carry {name:?}, got no [[package]] with \
             that name"
        )];
    };
    let mut reports = Vec::new();
    if versions.len() != 1 {
        reports.push(format!(
            "case {case:?}: expected exactly 1 locked copy of {name:?}, got {} ({versions:?}) \
             — a second copy puts another one in the build",
            versions.len()
        ));
    }
    let (wording, release) = (bound.wording(), bound.release());
    for version in versions {
        match bound.admits(version) {
            Some(true) => {}
            Some(false) => reports.push(format!(
                "case {case:?}: expected the locked {name:?} to be {wording} {release}, got \
                 {version:?}"
            )),
            None => reports.push(format!(
                "case {case:?}: expected the locked {name:?} to name a release that can be \
                 compared to {release}, got {version:?}"
            )),
        }
    }
    reports
}

/// How many requirements on the crates in `crates` the manifest declares, and one report per
/// requirement that carries no version at all, that does not accept `locked_version`, or that
/// would admit `refused` — the release the pin exists to keep out of the build.
///
/// The companion of `locked_copy_reports` on the manifest side, shared for the same reason: a
/// pin that only the lock states is a pin a re-resolve can undo, so each suite pairs its
/// crates' requirements with the copy the lock carries through this one reading.
///
/// A declaration that defers to the root `[workspace.dependencies]` is read through the entry it
/// defers to; one that defers to a table not declaring it resolves to nothing to read here and is
/// left to `every_dependency_a_workspace_manifest_declares_is_locked`, which reports that state.
pub(crate) fn pinned_requirement_reports(
    manifest_path: &str,
    manifest: &str,
    workspace_deps: &BTreeMap<String, Declared>,
    crates: &[&str],
    locked_version: &str,
    refused: &str,
) -> (usize, Vec<String>) {
    let mut declared = 0usize;
    let mut reports = Vec::new();
    for (table, key, spec) in dependencies_in(manifest, is_member_dependency_table) {
        let Some(resolved) = resolve_declared(&key, &spec, workspace_deps) else {
            continue;
        };
        // The crate the key resolves to, so one declared under an alias is read by the name the
        // lock knows rather than by the name the manifest chose.
        let crate_name = locked_crate_name(&key, &resolved);
        if !crates.contains(&crate_name.as_str()) {
            continue;
        }
        declared += 1;
        let Some(req) = &resolved.req else {
            reports.push(format!(
                "case {manifest_path:?} {table} -> {crate_name:?}: expected the pinned crate to \
                 carry a version requirement, got {resolved:?}"
            ));
            continue;
        };
        if !accepts(req, locked_version) {
            reports.push(format!(
                "case {manifest_path:?} {table} -> {crate_name:?}: expected the requirement \
                 {req:?} to accept the locked {locked_version}, it does not"
            ));
        }
        if accepts(req, refused) {
            reports.push(format!(
                "case {manifest_path:?} {table} -> {crate_name:?}: expected the requirement \
                 {req:?} to refuse {refused}, it accepts it"
            ));
        }
    }
    (declared, reports)
}

/// Every crate directory directly under `directory` (relative to the workspace root) that
/// holds a manifest, as the path to read it by — relative to that root — and its text, in
/// path order.
///
/// The ONE walk of a crate-holding directory in this target: the suites that read the
/// manifests of crates the root `members` array does not list — `vendor/`'s path
/// dependencies, and the crates under `services/` that are path dependencies of a member —
/// share it rather than each walking a directory the same way.
pub(crate) fn crate_manifests_under(directory: &str) -> Vec<(String, String)> {
    let root = workspace_root();
    let path = root.join(directory);
    let mut crates: Vec<String> = fs::read_dir(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("read an entry of {}: {error}", path.display()))
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    crates.sort();
    crates
        .into_iter()
        .filter_map(|name| {
            let manifest = format!("{directory}/{name}/Cargo.toml");
            fs::read_to_string(root.join(&manifest))
                .ok()
                .map(|text| (manifest, text))
        })
        .collect()
}

/// The declaration the lock is resolved from: the root `[workspace.dependencies]` entry a
/// `workspace = true` declaration defers to, or the declaration itself.
///
/// `None` where it defers to a root table that does not declare it, which leaves nothing to
/// pair with the lock.
pub(crate) fn resolve_declared(
    dependency: &str,
    spec: &Declared,
    workspace_deps: &BTreeMap<String, Declared>,
) -> Option<Declared> {
    if spec.from_workspace {
        workspace_deps.get(dependency).cloned()
    } else {
        Some(spec.clone())
    }
}

/// The name the LOCK knows a declaration by: the crate `package = "…"` renames it to, or the
/// name the manifest declares it under when it renames nothing.
pub(crate) fn locked_crate_name(dependency: &str, resolved: &Declared) -> String {
    resolved
        .package
        .clone()
        .unwrap_or_else(|| dependency.to_string())
}

/// One report per way the committed lock fails to cover the crate `resolved` requires under
/// the name `dependency`: no `[[package]]` carries that crate, or none of the versions it does
/// carry is one the requirement accepts. Either stops a `--locked` resolve.
///
/// This is the ONE reading of lock coverage in this target. A sibling pairing a narrower set of
/// declarations with the lock — `router_suites_are_declared_by_automod`, for the dependency a
/// test layout brings with it — reads it through here rather than through a second copy of the
/// rule that could drift from this one.
pub(crate) fn lock_coverage_reports(
    dependency: &str,
    resolved: &Declared,
    locked: &Locked,
) -> Vec<String> {
    let crate_name = locked_crate_name(dependency, resolved);
    let Some(versions) = locked.get(&crate_name) else {
        return vec![format!(
            "expected Cargo.lock to carry the crate {crate_name:?}, got no [[package]] with \
             that name"
        )];
    };
    match &resolved.req {
        Some(req) if !versions.iter().any(|version| accepts(req, version)) => vec![format!(
            "expected a locked version of {crate_name:?} that {req:?} accepts, got {versions:?}"
        )],
        _ => Vec::new(),
    }
}

/// How many dependencies `manifest` declares, and one report per dependency the lock does
/// not carry at a version the manifest's requirement accepts.
fn unlocked_dependencies(
    manifest_path: &str,
    manifest: &str,
    workspace_deps: &BTreeMap<String, Declared>,
    locked: &Locked,
) -> (usize, Vec<String>) {
    let declared = dependencies_in(manifest, is_member_dependency_table);
    let mut reports = Vec::new();
    for (table, dependency, spec) in &declared {
        let Some(resolved) = resolve_declared(dependency, spec, workspace_deps) else {
            reports.push(format!(
                "case {manifest_path:?} {table} -> {dependency:?}: declared \
                 `workspace = true`, expected the root [workspace.dependencies] to \
                 declare it, got nothing"
            ));
            continue;
        };
        for report in lock_coverage_reports(dependency, &resolved, locked) {
            reports.push(format!(
                "case {manifest_path:?} {table} -> {dependency:?}: {report}"
            ));
        }
    }
    (declared.len(), reports)
}

/// The committed lock's text.
pub(crate) fn committed_lock() -> String {
    fs::read_to_string(workspace_root().join("Cargo.lock")).expect("read Cargo.lock")
}

/// The root manifest's text, and every member's path (relative to the root) and text.
pub(crate) fn root_and_member_manifests() -> (String, Vec<(String, String)>) {
    let root = workspace_root();
    let root_manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read Cargo.toml");
    let members = workspace_members(&root_manifest);
    assert!(
        members.len() > 1,
        "the root manifest must list the workspace members, read {} of them",
        members.len()
    );
    let manifests = members
        .into_iter()
        .map(|member| {
            let path = root.join(&member).join("Cargo.toml");
            let manifest = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            (format!("{member}/Cargo.toml"), manifest)
        })
        .collect();
    (root_manifest, manifests)
}

/// The root manifest, the lock's packages, and every member's manifest text.
fn workspace() -> (String, Locked, Vec<(String, String)>) {
    let (root_manifest, manifests) = root_and_member_manifests();
    (root_manifest, locked_packages(&committed_lock()), manifests)
}

#[test]
fn every_workspace_member_is_a_locked_package() {
    let (_, locked, manifests) = workspace();
    for (manifest, text) in &manifests {
        let name = package_name(text)
            .unwrap_or_else(|| panic!("case {manifest:?}: the manifest names no [package]"));
        assert!(
            locked.contains_key(&name),
            "case {manifest:?}: expected Cargo.lock to carry the member package {name:?}, \
             got no [[package]] with that name"
        );
    }
}

#[test]
fn every_dependency_a_workspace_manifest_declares_is_locked() {
    let (root_manifest, locked, manifests) = workspace();
    let workspace_deps = workspace_dependency_table(&root_manifest);
    let mut checked = 0usize;
    let mut reports = Vec::new();
    for (manifest, text) in &manifests {
        let (declared, unlocked) =
            unlocked_dependencies(manifest, text, &workspace_deps, &locked);
        checked += declared;
        reports.extend(unlocked);
    }
    assert!(
        reports.is_empty(),
        "expected every declared dependency to be locked, got {} that are not, so the \
         release build cannot resolve with --locked: {reports:#?}",
        reports.len()
    );
    assert!(
        checked > 100,
        "expected the workspace's manifests to declare dependencies to check, got {checked}"
    );
}

#[test]
fn no_manifest_declares_a_dependency_in_a_form_the_suite_cannot_read() {
    let (root_manifest, _, manifests) = workspace();
    let mut reports = unreadable_dependency_forms("Cargo.toml", &root_manifest);
    for (manifest, text) in &manifests {
        reports.extend(unreadable_dependency_forms(manifest, text));
    }
    assert!(
        reports.is_empty(),
        "expected every dependency to be declared on its own key in a `[…dependencies]` \
         table, which is what the coverage check above reads, got {} that are not: \
         {reports:#?}",
        reports.len()
    );
    // Every form that would hide a dependency from the coverage check, so this guard is
    // known to fail on each rather than to pass over it, and the two spellings next to them
    // that are NOT dependencies and must stay accepted.
    let cases = [
        ("a sub-table", "[dependencies.absent-crate]\nversion = \"1\"\n", 1),
        ("a dev-dependency sub-table", "[dev-dependencies.absent-crate]\nversion = \"1\"\n", 1),
        ("a dotted key in a dependency table", "[dependencies]\nabsent-crate.workspace = true\n", 1),
        ("a dotted key in a target dependency table", "[target.'cfg(unix)'.dependencies]\nabsent-crate.version = \"1\"\n", 1),
        ("a dotted package field", "[package]\nversion.workspace = true\n", 0),
        ("a dependency on its own key", "[dependencies]\nlocked-crate = { workspace = true }\n", 0),
    ];
    for case in cases {
        let (name, manifest, expected) = case;
        let reports = unreadable_dependency_forms(name, manifest);
        assert_eq!(
            reports.len(),
            expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}

#[test]
fn an_unlocked_dependency_is_reported() {
    // One synthetic manifest per way a declaration reaches the lock, against a lock that
    // carries `locked-crate 1.2.3` and the renamed `renamed-crate 0.4.0` only.
    let locked: Locked = [
        ("locked-crate".to_string(), vec!["1.2.3".to_string()]),
        ("renamed-crate".to_string(), vec!["0.4.0".to_string()]),
    ]
    .into_iter()
    .collect();
    let workspace_deps = workspace_dependency_table(
        "[workspace.dependencies]\n\
         locked-crate = \"1\"\n\
         alias = { path = \"vendor/renamed\", package = \"renamed-crate\" }\n",
    );
    let cases = [
        ("a locked crate at an accepted version", "[dependencies]\nlocked-crate = \"1.2\"\n", 0),
        ("a crate the lock does not carry", "[dependencies]\nabsent-crate = \"1\"\n", 1),
        ("a locked crate below the requirement", "[dependencies]\nlocked-crate = \"1.3\"\n", 1),
        ("a locked crate in another major", "[dependencies]\nlocked-crate = \"2\"\n", 1),
        ("an exact pin on the locked version", "[dependencies]\nlocked-crate = \"=1.2.3\"\n", 0),
        ("an exact pin on another version", "[dependencies]\nlocked-crate = \"=1.2.4\"\n", 1),
        ("a dev-dependency the lock does not carry", "[dev-dependencies]\nabsent-crate = \"1\"\n", 1),
        ("a build-dependency the lock carries", "[build-dependencies]\nlocked-crate = \"1\"\n", 0),
        ("a target dependency the lock does not carry", "[target.'cfg(unix)'.dependencies]\nabsent-crate = \"1\"\n", 1),
        ("a workspace dependency the root table declares", "[dependencies]\nlocked-crate = { workspace = true }\n", 0),
        ("a renamed workspace dependency", "[dependencies]\nalias = { workspace = true }\n", 0),
        ("a workspace dependency the root table omits", "[dependencies]\nabsent-crate = { workspace = true }\n", 1),
        ("a path dependency with no requirement", "[dependencies]\nlocked-crate = { path = \"../locked\" }\n", 0),
        ("a package field, not a dependency", "[package]\nname = \"absent-crate\"\nversion.workspace = true\n", 0),
        ("a multi-line declaration of a locked crate", "[dependencies]\nlocked-crate = { version = \"1\", features = [\n    \"one\",\n] }\n", 0),
        ("a multi-line declaration below the requirement", "[dependencies]\nlocked-crate = { version = \"1.3\", features = [\n    \"one\",\n] }\n", 1),
        ("a multi-line declaration of an absent crate", "[dependencies]\nabsent-crate = { version = \"1\", features = [\n    \"one\",\n] }\n", 1),
        ("one name required differently by two tables", "[dependencies]\nlocked-crate = \"1.3\"\n\n[dev-dependencies]\nlocked-crate = \"1\"\n", 1),
        ("a brace inside a value, before a dependency table", "[package]\ndescription = \"takes a { brace\"\n\n[dependencies]\nabsent-crate = \"1\"\n", 1),
    ];
    for case in cases {
        let (name, manifest, expected) = case;
        let (_, reports) = unlocked_dependencies(name, manifest, &workspace_deps, &locked);
        assert_eq!(
            reports.len(),
            expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}

#[test]
fn caret_and_exact_requirements_accept_what_cargo_accepts() {
    // Rows from cargo's documented default (caret) and `=` requirement semantics: the
    // upper bound follows the leftmost non-zero component spelled out. The last two rows
    // are forms this suite deliberately does not read, and so reports accepted.
    let cases = [
        ("1", "1.0.0", true),
        ("1", "1.9.2", true),
        ("1", "2.0.0", false),
        ("1", "0.9.9", false),
        ("^1.2.3", "1.2.3", true),
        ("^1.2.3", "1.2.2", false),
        ("0.22", "0.22.0", true),
        ("0.22", "0.22.9", true),
        ("0.22", "0.23.0", false),
        ("0.22", "0.21.9", false),
        ("0.0.3", "0.0.3", true),
        ("0.0.3", "0.0.4", false),
        ("0", "0.9.1", true),
        ("0", "1.0.0", false),
        ("=3.20.3", "3.20.3", true),
        ("=3.20.3", "3.20.4", false),
        ("=3.20.3", "3.21.0", false),
        ("*", "1.2.3", true),
        (">=1, <3", "2.0.0", true),
    ];
    for case in cases {
        let (req, version, expected) = case;
        let actual = accepts(req, version);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the requirement to accept the version = {expected}, \
             got {actual}"
        );
    }
}
