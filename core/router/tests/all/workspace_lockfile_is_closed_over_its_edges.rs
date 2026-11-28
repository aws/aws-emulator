// SPDX-License-Identifier: CC0-1.0

//! The committed `Cargo.lock` is CLOSED: nothing in it points outside it, nothing in it is
//! unreachable, and the manifests of the path dependencies it resolves are covered by it.
//!
//! Every platform build resolves with `--locked`, which lets cargo write nothing: when the
//! resolve it computes differs from the committed file in ANY respect it stops the build
//! before a crate compiles, with `cannot update the lock file ... because --locked was
//! passed`. A lock cargo wrote is exactly the resolve graph rooted at the workspace members,
//! so three differences stop that build while every crate a member's manifest names is still
//! present at an accepted version, which is as far as the sibling suites read:
//!
//! * an EDGE naming a crate no `[[package]]` carries — cargo has to add the package;
//! * a `[[package]]` no member reaches through those edges — cargo has to drop it; and
//! * a dependency of a crate in the resolve that is built out of this checkout by PATH rather
//!   than taken off a registry, whose manifest no sibling suite reads, because the root
//!   manifest's `members` array does not list it.
//!
//! The third is how a platform build fails on a dependency the host build never needs: the
//! path dependencies carry the only platform-specific dependency tables in this checkout, so
//! a crate just one target resolves is locked for every target or for none.
//!
//! RULE: the committed lock has to be the whole resolve — closed under its own edges, reached
//! from the workspace members, and covering every manifest the resolve reads — so that no
//! `--locked` invocation has anything to write.
//!
//! It reads the committed files only; it resolves nothing, spawns nothing and reaches no
//! network. Both formats are read through `workspace_lockfile_covers_every_manifest`, which
//! owns the one reader of each, and every check here is a function over those records, driven
//! once over the committed files and once over synthetic ones, so each is known to REPORT
//! what it is there to catch rather than only to pass on the tree as it stands.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::workspace_lockfile_covers_every_manifest::{
    committed_lock, crate_manifests_under, dependencies_in, is_member_dependency_table,
    lock_coverage_reports, lock_entries, locked_crate_name, locked_packages, package_name,
    resolve_declared, root_and_member_manifests, Declared, LockEntry, Locked,
};
use crate::workspace_lockfile_records_each_member_dependency::{
    source_free_entries, LEAST_EXPECTED_PACKAGES,
};

/// How many edges a read of the committed lock must return before the closure check over them
/// means anything: every one of the hundreds of packages it carries depends on something, so a
/// smaller number is a reader that read the packages and not their edges.
const LEAST_EXPECTED_EDGES: usize = 1_000;

/// Where this checkout keeps the crates its members depend on by PATH: the framework crates
/// every service crate is built against. They are resolved into the same lock as the members —
/// a path dependency is a package of the resolve — while the root manifest's `members` array
/// does not list them, so no sibling suite reads their manifests.
const PATH_DEPENDENCY_DIRECTORY: &str = "vendor";

/// The dependency tables of a path dependency that the resolve READS. Cargo resolves a
/// non-member's normal, build and platform-specific dependencies; it never resolves its
/// dev-dependencies, which belong to tests that are not built from this workspace, so a crate
/// one of those names is absent from the lock by design and is not paired here.
fn is_resolved_dependency_table(section: &str) -> bool {
    is_member_dependency_table(section) && !section.contains("dev-dependencies")
}

/// The lock's edges merged by crate NAME: a crate locked at more than one version is one node
/// carrying every edge any of its versions writes.
///
/// An edge is written by name, or by name and version where the lock holds several versions of
/// the crate, and the reader keeps the name — so a name is the only identity this closure has.
/// That makes it conservative: a crate reached at one version counts as reached at every
/// version the lock carries for it, and an edge naming a carried crate at a version the lock
/// does not carry reads as closed. It can therefore only report a crate name the lock
/// genuinely points outside itself for, or genuinely reaches from nothing.
fn edges_by_name(entries: &[LockEntry]) -> BTreeMap<&str, BTreeSet<&str>> {
    let mut edges: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for entry in entries {
        let node = edges.entry(entry.name.as_str()).or_default();
        for dependency in &entry.dependencies {
            node.insert(dependency.as_str());
        }
    }
    edges
}

/// One report per edge the lock writes that names a crate no `[[package]]` of it carries.
/// Cargo can only answer such an edge by resolving the crate and writing the package it
/// resolved, which `--locked` forbids.
fn dangling_edge_reports(entries: &[LockEntry]) -> Vec<String> {
    let carried: BTreeSet<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    let mut reports = Vec::new();
    for entry in entries {
        for dependency in &entry.dependencies {
            if !carried.contains(dependency.as_str()) {
                reports.push(format!(
                    "case {:?} {:?} -> {dependency:?}: expected Cargo.lock to carry the crate \
                     this edge names as a [[package]] of its own, got none, so a --locked \
                     build cannot resolve from this lock",
                    entry.name, entry.version
                ));
            }
        }
    }
    reports
}

/// The crate names `roots` reaches by following the lock's edges.
fn reached<'entries>(
    entries: &'entries [LockEntry],
    roots: &BTreeSet<String>,
) -> BTreeSet<&'entries str> {
    let edges = edges_by_name(entries);
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    for name in edges.keys() {
        if roots.contains(*name) && seen.insert(*name) {
            queue.push_back(*name);
        }
    }
    while let Some(name) = queue.pop_front() {
        for dependency in edges.get(name).into_iter().flatten() {
            if seen.insert(*dependency) {
                queue.push_back(*dependency);
            }
        }
    }
    seen
}

/// One report per `[[package]]` of the lock that no root reaches. Cargo's resolve holds only
/// what the workspace reaches, so a package nothing reaches is one cargo would drop — another
/// write `--locked` forbids.
fn unreached_package_reports(entries: &[LockEntry], roots: &BTreeSet<String>) -> Vec<String> {
    let reached = reached(entries, roots);
    entries
        .iter()
        .filter(|entry| !reached.contains(entry.name.as_str()))
        .map(|entry| {
            format!(
                "case {:?} {:?}: expected every [[package]] in Cargo.lock to be reached from a \
                 workspace member through the lock's own edges, got one nothing reaches, which \
                 cargo's resolve drops, so a --locked build cannot resolve from this lock",
                entry.name, entry.version
            )
        })
        .collect()
}

/// Whether the manifest was paired with the lock at all, and one report per way the lock fails
/// to record a dependency it declares: a crate the lock carries at no accepted version, and a
/// crate the lock carries but does not list among this package's edges. A manifest for a crate
/// the resolve does not hold is paired with nothing — a path dependency the workspace does not
/// depend on is absent from the lock by design.
fn path_dependency_reports(
    manifest_path: &str,
    manifest: &str,
    source_free: &BTreeMap<String, LockEntry>,
    locked: &Locked,
) -> (bool, Vec<String>) {
    let Some(name) = package_name(manifest) else {
        return (
            false,
            vec![format!(
                "case {manifest_path:?}: expected the manifest to name a [package], got none"
            )],
        );
    };
    let Some(entry) = source_free.get(&name) else {
        return (false, Vec::new());
    };
    // A crate outside the workspace has no root `[workspace.dependencies]` to defer to, so the
    // table it would resolve a `workspace = true` through is empty here, and a declaration that
    // asks for one is reported rather than passed over.
    let workspace_deps: BTreeMap<String, Declared> = BTreeMap::new();
    let mut reports = Vec::new();
    for (table, dependency, spec) in dependencies_in(manifest, is_resolved_dependency_table) {
        let Some(resolved) = resolve_declared(&dependency, &spec, &workspace_deps) else {
            reports.push(format!(
                "case {manifest_path:?} {table} -> {dependency:?}: declared `workspace = true`, \
                 which a crate outside the workspace has no root table to resolve through"
            ));
            continue;
        };
        for report in lock_coverage_reports(&dependency, &resolved, locked) {
            reports.push(format!(
                "case {manifest_path:?} {table} -> {dependency:?}: {report}"
            ));
        }
        let crate_name = locked_crate_name(&dependency, &resolved);
        if !entry.dependencies.contains(&crate_name) {
            reports.push(format!(
                "case {manifest_path:?} {table} -> {crate_name:?}: expected the [[package]] \
                 {name:?} in Cargo.lock to list it among its {} dependencies, got none, so a \
                 --locked build cannot resolve this manifest",
                entry.dependencies.len()
            ));
        }
    }
    (true, reports)
}

/// Every path dependency's manifest in this checkout, as the path to read it by and its text.
fn path_dependency_manifests() -> Vec<(String, String)> {
    crate_manifests_under(PATH_DEPENDENCY_DIRECTORY)
}

/// The committed lock's `[[package]]` records, and the package name of every workspace member —
/// the roots cargo resolves the graph from.
fn committed_lock_and_roots() -> (Vec<LockEntry>, BTreeSet<String>) {
    let entries = lock_entries(&committed_lock());
    assert!(
        entries.len() > LEAST_EXPECTED_PACKAGES,
        "expected Cargo.lock to carry the workspace's packages, read {} of them",
        entries.len()
    );
    let (_root_manifest, manifests) = root_and_member_manifests();
    let roots = manifests
        .iter()
        .map(|(manifest, text)| {
            package_name(text)
                .unwrap_or_else(|| panic!("case {manifest:?}: the manifest names no [package]"))
        })
        .collect();
    (entries, roots)
}

#[test]
fn every_edge_the_committed_lock_writes_names_a_package_it_carries() {
    let (entries, _) = committed_lock_and_roots();
    let reports = dangling_edge_reports(&entries);
    assert!(
        reports.is_empty(),
        "expected every edge in Cargo.lock to name a [[package]] of the same lock, got {} that \
         do not, so the platform build cannot resolve with --locked: {reports:#?}",
        reports.len()
    );
    let edges: usize = entries.iter().map(|entry| entry.dependencies.len()).sum();
    assert!(
        edges > LEAST_EXPECTED_EDGES,
        "expected the lock's packages to carry the edges of the resolve, counted {edges}"
    );
}

#[test]
fn every_package_the_committed_lock_carries_is_reached_from_a_workspace_member() {
    let (entries, roots) = committed_lock_and_roots();
    let reports = unreached_package_reports(&entries, &roots);
    assert!(
        reports.is_empty(),
        "expected every [[package]] in Cargo.lock to be reached from a workspace member, got {} \
         that are not, so the platform build cannot resolve with --locked: {reports:#?}",
        reports.len()
    );
}

#[test]
fn every_dependency_a_path_dependency_in_the_resolve_declares_is_locked() {
    let (entries, _) = committed_lock_and_roots();
    let source_free = source_free_entries(&entries);
    let locked = locked_packages(&committed_lock());
    let manifests = path_dependency_manifests();
    assert!(
        !manifests.is_empty(),
        "expected the {PATH_DEPENDENCY_DIRECTORY:?} directory to hold the path dependencies' \
         manifests, read none"
    );
    let mut paired = 0usize;
    let mut reports = Vec::new();
    for (manifest, text) in &manifests {
        let (was_paired, manifest_reports) =
            path_dependency_reports(manifest, text, &source_free, &locked);
        paired += usize::from(was_paired);
        reports.extend(manifest_reports);
    }
    assert!(
        reports.is_empty(),
        "expected Cargo.lock to record every dependency the path dependencies of the resolve \
         declare, got {} it does not, so the platform build cannot resolve with --locked: \
         {reports:#?}",
        reports.len()
    );
    assert!(
        paired == manifests.len() && paired > 1,
        "expected every one of the {} path-dependency manifests to be paired with its \
         [[package]] in Cargo.lock, paired {paired}",
        manifests.len()
    );
}

/// A lock in order: one member crate, the path dependency and the registry crate it depends on,
/// and the crate those two share, which is locked at two versions so an edge carrying a version
/// is read here too.
const CLOSED_LOCK: &str = r#"
version = 4

[[package]]
name = "member-crate"
version = "0.1.0"
dependencies = [
 "path-crate",
 "shared-crate 0.10.9",
]

[[package]]
name = "path-crate"
version = "0.1.0"
dependencies = [
 "locked-crate",
]

[[package]]
name = "locked-crate"
version = "1.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1111"
dependencies = [
 "shared-crate 0.9.9",
]

[[package]]
name = "shared-crate"
version = "0.9.9"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2222"

[[package]]
name = "shared-crate"
version = "0.10.9"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "3333"
"#;

/// `CLOSED_LOCK` with `extra` appended, so a row states only how it differs from a lock in
/// order.
fn lock_with(extra: &str) -> String {
    format!("{CLOSED_LOCK}{extra}")
}

#[test]
fn an_edge_naming_a_package_the_lock_does_not_carry_is_reported() {
    // Rows differ along WHERE the edge is written and how many of them point outside the lock;
    // the last two are edges that stay closed, including one written with a version the lock
    // does not carry for a crate it does carry, which this reader identifies by name.
    let cases = [
        ("a lock in order", CLOSED_LOCK.to_string(), 0),
        (
            "a member edge naming nothing",
            CLOSED_LOCK.replace("\"path-crate\",", "\"absent-crate\","),
            1,
        ),
        (
            "a path-dependency edge naming nothing",
            CLOSED_LOCK.replace("\"locked-crate\",", "\"absent-crate\","),
            1,
        ),
        (
            "a registry edge naming nothing",
            CLOSED_LOCK.replace("\"shared-crate 0.9.9\",", "\"absent-crate\","),
            1,
        ),
        (
            "two edges naming nothing",
            CLOSED_LOCK
                .replace("\"path-crate\",", "\"absent-crate\",")
                .replace("\"locked-crate\",", "\"another-absent-crate\","),
            2,
        ),
        (
            "an added package whose own edge names nothing",
            lock_with(
                "\n[[package]]\nname = \"added-crate\"\nversion = \"2.0.0\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
                 checksum = \"4444\"\ndependencies = [\n \"absent-crate\",\n]\n",
            ),
            1,
        ),
        (
            "an edge on a version the lock does not carry of a crate it does",
            CLOSED_LOCK.replace("\"shared-crate 0.9.9\",", "\"shared-crate 0.1.0\","),
            0,
        ),
    ];
    for case in &cases {
        let (name, lock, expected) = case;
        let reports = dangling_edge_reports(&lock_entries(lock));
        assert_eq!(
            reports.len(),
            *expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}

#[test]
fn a_package_no_member_reaches_is_reported() {
    let member: BTreeSet<String> = ["member-crate".to_string()].into_iter().collect();
    let nothing: BTreeSet<String> = BTreeSet::new();
    let other: BTreeSet<String> = ["another-crate".to_string()].into_iter().collect();
    // Rows differ along how far from the member the unreached package sits, how many there are,
    // and which packages count as roots at all.
    let cases = [
        ("a lock in order", CLOSED_LOCK.to_string(), &member, 0),
        (
            "a package nothing depends on",
            lock_with(
                "\n[[package]]\nname = \"orphan-crate\"\nversion = \"1.0.0\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
                 checksum = \"4444\"\n",
            ),
            &member,
            1,
        ),
        (
            "two packages reachable only from each other",
            lock_with(
                "\n[[package]]\nname = \"orphan-crate\"\nversion = \"1.0.0\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
                 checksum = \"4444\"\ndependencies = [\n \"orphan-dependency\",\n]\n\
                 \n[[package]]\nname = \"orphan-dependency\"\nversion = \"1.0.0\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
                 checksum = \"5555\"\n",
            ),
            &member,
            2,
        ),
        (
            // The crate below it is orphaned with it; the crate BELOW that one is not, because
            // the member reaches another version of it, and this closure identifies a crate by
            // name.
            "the edge to the path dependency dropped, orphaning it and the crate below it",
            CLOSED_LOCK.replace(" \"path-crate\",\n", ""),
            &member,
            2,
        ),
        (
            "a second version of a shared crate, reached because the first is",
            lock_with(
                "\n[[package]]\nname = \"shared-crate\"\nversion = \"0.11.0\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
                 checksum = \"4444\"\n",
            ),
            &member,
            0,
        ),
        ("no roots at all", CLOSED_LOCK.to_string(), &nothing, 5),
        (
            "a root the lock does not carry",
            CLOSED_LOCK.to_string(),
            &other,
            5,
        ),
    ];
    for case in &cases {
        let (name, lock, roots, expected) = case;
        let reports = unreached_package_reports(&lock_entries(lock), roots);
        assert_eq!(
            reports.len(),
            *expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}

#[test]
fn a_path_dependency_declaration_the_lock_does_not_record_is_reported() {
    let entries = lock_entries(CLOSED_LOCK);
    let source_free = source_free_entries(&entries);
    let locked = locked_packages(CLOSED_LOCK);
    // Each manifest defines `path-crate`, whose one locked edge is `locked-crate 1.2.3`, so the
    // rows differ only in what it declares, in which table, and under which name.
    let path_crate = "[package]\nname = \"path-crate\"\nversion = \"0.1.0\"\n\n";
    let cases = [
        (
            "the one edge, declared plainly",
            format!("{path_crate}[dependencies]\nlocked-crate = \"1\"\n"),
            true,
            0,
        ),
        (
            "the one edge, renamed by the declaration",
            format!(
                "{path_crate}[dependencies]\nalias = {{ path = \"../locked\", \
                 package = \"locked-crate\", version = \"*\" }}\n"
            ),
            true,
            0,
        ),
        (
            "the one edge, declared for one platform",
            format!("{path_crate}[target.'cfg(windows)'.dependencies]\nlocked-crate = \"1\"\n"),
            true,
            0,
        ),
        (
            "the one edge, and a crate the lock does not carry",
            format!("{path_crate}[dependencies]\nlocked-crate = \"1\"\nabsent-crate = \"0.2\"\n"),
            true,
            2,
        ),
        (
            "the one edge, and a crate the lock carries at no accepted version",
            format!("{path_crate}[dependencies]\nlocked-crate = \"2\"\n"),
            true,
            1,
        ),
        (
            "the one edge, and a crate the lock carries but lists no edge to",
            format!("{path_crate}[dependencies]\nlocked-crate = \"1\"\nshared-crate = \"0.10\"\n"),
            true,
            1,
        ),
        (
            "a dev-dependency the lock does not carry, which the resolve never reads",
            format!(
                "{path_crate}[dependencies]\nlocked-crate = \"1\"\n\n\
                 [dev-dependencies]\nabsent-crate = \"1\"\n"
            ),
            true,
            0,
        ),
        (
            "a declaration deferred to a workspace table a crate outside the workspace has none of",
            format!("{path_crate}[dependencies]\nlocked-crate = {{ workspace = true }}\n"),
            true,
            1,
        ),
        (
            "a path dependency the resolve does not hold",
            "[package]\nname = \"absent-path-crate\"\n\n[dependencies]\nabsent-crate = \"1\"\n"
                .to_string(),
            false,
            0,
        ),
        (
            "a manifest that names no package",
            "[dependencies]\nlocked-crate = \"1\"\n".to_string(),
            false,
            1,
        ),
    ];
    for case in &cases {
        let (name, manifest, expected_paired, expected) = case;
        let (paired, reports) = path_dependency_reports(name, manifest, &source_free, &locked);
        assert_eq!(
            paired, *expected_paired,
            "case {name:?}: expected the manifest to be paired with the lock = \
             {expected_paired}, got {paired}"
        );
        assert_eq!(
            reports.len(),
            *expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}
