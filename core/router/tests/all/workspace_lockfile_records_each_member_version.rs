// SPDX-License-Identifier: CC0-1.0

//! The committed `Cargo.lock` locks every workspace member at the version its manifest
//! resolves to.
//!
//! A `--locked` build lets cargo write nothing, so the committed lock has to be the resolve
//! cargo would compute: a member the lock does not carry — one added to the root manifest's
//! `members` without refreshing the lock — and a member it carries at another version are the
//! same defect to cargo, which answers both with `cannot update the lock file ... because
//! --locked was passed` before a crate compiles.
//!
//! `workspace_lockfile_covers_every_manifest` pairs each member with the lock by NAME and
//! `workspace_lockfile_records_each_member_dependency` by its EDGES; a member record left at
//! the version it was first locked at satisfies both. This suite pins the remaining part of
//! the pair, the version, and reports a member the lock omits as the same kind of difference,
//! so the whole of a member's `[[package]]` identity is held by the workspace's own tests
//! instead of by a platform build nobody can rerun locally.
//!
//! A member inherits its version with `version.workspace = true`, from the root manifest's
//! `[workspace.package]`, or spells one out itself; both forms are resolved here and paired
//! with what the lock records.
//!
//! It reads the two committed files only; it resolves nothing, spawns nothing and reaches no
//! network. Both are read through the readers `workspace_lockfile_covers_every_manifest` owns
//! — `lock_entries` for the lock's records, `root_and_member_manifests` for the texts — and
//! the lock's source-free records through `source_free_entries`, so this suite adds no second
//! reader of either format. Every check is a function over those records, driven once over
//! the committed files and once over synthetic ones, so it is known to REPORT what it is here
//! to catch rather than only to pass on the tree as it stands.

use std::collections::BTreeMap;

use crate::workspace_lockfile_covers_every_manifest::{
    committed_lock, lock_entries, package_name, root_and_member_manifests, unquote, LockEntry,
};
use crate::workspace_lockfile_records_each_member_dependency::{
    source_free_entries, LEAST_EXPECTED_PACKAGES,
};

/// How many members a read of the root manifest must return before a check over them means
/// anything: this workspace has hundreds, so a smaller number is a reader that stopped early.
const LEAST_EXPECTED_MEMBERS: usize = 100;

/// The version a `[package]` declares: a literal, or the root `[workspace.package]` entry it
/// inherits with `version.workspace = true`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DeclaredVersion {
    Literal(String),
    FromWorkspace,
}

/// The version the `[package]` of `manifest` declares, in whichever of the two forms it uses.
fn declared_version(manifest: &str) -> Option<DeclaredVersion> {
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[package]";
        } else if inside {
            // The inherited forms first: `version = { workspace = true }` also begins with
            // `version = `, and reading it as a literal would pair the lock with a table.
            if line.starts_with("version.workspace") || line.starts_with("version = { workspace") {
                return Some(DeclaredVersion::FromWorkspace);
            }
            if let Some(value) = line.strip_prefix("version = ") {
                return Some(DeclaredVersion::Literal(unquote(value).to_string()));
            }
        }
    }
    None
}

/// The `version` of the root manifest's `[workspace.package]` table, which every member that
/// inherits its version resolves to.
fn workspace_package_version(root_manifest: &str) -> Option<String> {
    let mut inside = false;
    for line in root_manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[workspace.package]";
        } else if inside {
            if let Some(value) = line.strip_prefix("version = ") {
                return Some(unquote(value).to_string());
            }
        }
    }
    None
}

/// One report per way the lock's record for the member `manifest` defines differs from the
/// identity that manifest resolves to: no record built out of this checkout under its name,
/// which is a member cargo would have to add, and a record at another version, which cargo
/// would have to rewrite. A manifest that names no package, or no version, leaves nothing to
/// pair and is reported as such.
fn version_reports(
    manifest_path: &str,
    manifest: &str,
    workspace_version: &str,
    locked: &BTreeMap<String, LockEntry>,
) -> Vec<String> {
    let Some(name) = package_name(manifest) else {
        return vec![format!(
            "case {manifest_path:?}: expected the manifest to name a [package], got none"
        )];
    };
    let expected = match declared_version(manifest) {
        Some(DeclaredVersion::Literal(version)) => version,
        Some(DeclaredVersion::FromWorkspace) => workspace_version.to_string(),
        None => {
            return vec![format!(
                "case {manifest_path:?}: expected the [package] {name:?} to declare a version, \
                 either its own or the workspace's, got none"
            )]
        }
    };
    let Some(entry) = locked.get(&name) else {
        return vec![format!(
            "case {manifest_path:?}: expected Cargo.lock to carry the member package {name:?} at \
             {expected:?} as a [[package]] built out of this checkout, got none, so a --locked \
             build cannot resolve this manifest"
        )];
    };
    if entry.version == expected {
        return Vec::new();
    }
    vec![format!(
        "case {manifest_path:?}: expected the [[package]] {name:?} in Cargo.lock to be locked at \
         the version its manifest resolves to, {expected:?}, got {:?}, so a --locked build \
         cannot resolve this manifest",
        entry.version
    )]
}

/// The root manifest's text, the lock's source-free records by crate name, and every member's
/// path and text.
fn workspace() -> (String, BTreeMap<String, LockEntry>, Vec<(String, String)>) {
    let (root_manifest, manifests) = root_and_member_manifests();
    let entries = lock_entries(&committed_lock());
    assert!(
        entries.len() > LEAST_EXPECTED_PACKAGES,
        "expected Cargo.lock to carry the workspace's packages, read {} of them",
        entries.len()
    );
    assert!(
        manifests.len() > LEAST_EXPECTED_MEMBERS,
        "expected the root manifest to list the workspace's members, read {} of them",
        manifests.len()
    );
    (root_manifest, source_free_entries(&entries), manifests)
}

#[test]
fn every_workspace_member_is_locked_at_the_version_its_manifest_resolves_to() {
    let (root_manifest, locked, manifests) = workspace();
    let workspace_version = workspace_package_version(&root_manifest).unwrap_or_else(|| {
        panic!("expected the root manifest's [workspace.package] to declare a version")
    });
    let mut reports = Vec::new();
    for (manifest, text) in &manifests {
        reports.extend(version_reports(manifest, text, &workspace_version, &locked));
    }
    assert!(
        reports.is_empty(),
        "expected every member to be locked at the version its manifest resolves to, got {} that \
         differ, so the release build cannot resolve with --locked: {reports:#?}",
        reports.len()
    );
}

/// A lock holding two member crates built out of a checkout — so no source and no checksum —
/// beside a registry crate of the same name as one of them, which is not a member record.
const SYNTHETIC_LOCK: &str = r#"
version = 4

[[package]]
name = "member-crate"
version = "0.1.0"

[[package]]
name = "other-member-crate"
version = "2.5.0"

[[package]]
name = "registry-crate"
version = "1.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1111"
"#;

#[test]
fn a_member_the_lock_omits_or_locks_at_another_version_is_reported() {
    let locked = source_free_entries(&lock_entries(SYNTHETIC_LOCK));
    // Rows differ along how the manifest states its version (inherited, its own), whether the
    // lock carries that member at all, and whether the version it carries is the one the
    // manifest resolves to. The workspace version is "0.1.0", which is what `member-crate` is
    // locked at, so an inherited version agrees for that member and not for the other.
    let cases = [
        (
            "a member inheriting the workspace version it is locked at",
            "[package]\nname = \"member-crate\"\nversion.workspace = true\n",
            0,
        ),
        (
            "a member inheriting the workspace version in the inline-table form",
            "[package]\nname = \"member-crate\"\nversion = { workspace = true }\n",
            0,
        ),
        (
            "a member spelling out the version it is locked at",
            "[package]\nname = \"other-member-crate\"\nversion = \"2.5.0\"\n",
            0,
        ),
        (
            "a member inheriting a version the lock disagrees with",
            "[package]\nname = \"other-member-crate\"\nversion.workspace = true\n",
            1,
        ),
        (
            "a member spelling out a version the lock disagrees with",
            "[package]\nname = \"member-crate\"\nversion = \"0.2.0\"\n",
            1,
        ),
        (
            "a member the lock does not carry — the defect a new member brings",
            "[package]\nname = \"absent-member-crate\"\nversion.workspace = true\n",
            1,
        ),
        (
            "a member whose name the lock carries only as a registry package",
            "[package]\nname = \"registry-crate\"\nversion = \"1.2.3\"\n",
            1,
        ),
        (
            "a manifest declaring no version",
            "[package]\nname = \"member-crate\"\nedition = \"2021\"\n",
            1,
        ),
        (
            "a manifest naming no package",
            "[dependencies]\nregistry-crate = \"1\"\n",
            1,
        ),
        (
            "a version line outside the [package] table",
            "[package]\nname = \"member-crate\"\n\n[dependencies]\nversion = \"9.9.9\"\n",
            1,
        ),
    ];
    for case in &cases {
        let (name, manifest, expected) = case;
        let reports = version_reports(name, manifest, "0.1.0", &locked);
        assert_eq!(
            reports.len(),
            *expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}

#[test]
fn the_declared_version_reader_reads_both_forms_and_the_workspace_table() {
    let cases = [
        (
            "an inherited version, dotted",
            "[package]\nname = \"a\"\nversion.workspace = true\n",
            Some(DeclaredVersion::FromWorkspace),
        ),
        (
            "an inherited version, inline table",
            "[package]\nname = \"a\"\nversion = { workspace = true }\n",
            Some(DeclaredVersion::FromWorkspace),
        ),
        (
            "a literal version",
            "[package]\nname = \"a\"\nversion = \"0.3.1\"\n",
            Some(DeclaredVersion::Literal("0.3.1".to_string())),
        ),
        (
            "a version only another table declares",
            "[package]\nname = \"a\"\n\n[dependencies]\nversion = \"9.9.9\"\n",
            None,
        ),
        ("no version at all", "[package]\nname = \"a\"\n", None),
    ];
    for case in &cases {
        let (name, manifest, expected) = case;
        let read = declared_version(manifest);
        assert_eq!(
            read, *expected,
            "case {name:?}: expected {expected:?}, got {read:?}"
        );
    }
    let root = "[workspace]\nmembers = [\n    \"a\",\n]\n\n[workspace.package]\nversion = \"4.2.0\"\nedition = \"2021\"\n";
    assert_eq!(
        workspace_package_version(root).as_deref(),
        Some("4.2.0"),
        "case the synthetic root manifest: expected the [workspace.package] version"
    );
    assert_eq!(
        workspace_package_version("[workspace]\nmembers = []\n\n[package]\nversion = \"4.2.0\"\n")
            .as_deref(),
        None,
        "case a root manifest with no [workspace.package]: expected no version to be read from \
         another table"
    );
}
