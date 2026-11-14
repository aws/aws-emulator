// SPDX-License-Identifier: CC0-1.0

//! The ZIP archive a build's input and its output travel in.
//!
//! # The rule this module implements
//!
//! The object a build's S3 source names is an archive: "the input artifact must be a ZIP
//! file" that CodeBuild unpacks into the build's working tree before the first command
//! runs, so a buildspec that reads a file of the repository reads it out of that archive.
//! The same form carries a build's output the other way: artifacts whose `packaging` is
//! `ZIP` are "a ZIP file that contains the build output" put at the destination as ONE
//! object, and the cache a project carries from one build into the next is the cached
//! paths in the same form.
//!
//! # What is read, and what is not
//!
//! The archive is caller-supplied bytes, so the walk is bounded here rather than trusted
//! to be small: how many entries one archive contributes, how large one entry may be and
//! how much the whole archive may expand to are this module's numbers
//! ([`MAX_ENTRIES`], [`MAX_ENTRY_BYTES`], [`MAX_EXPANDED_BYTES`]) and an entry past one of
//! them is left out rather than held. A directory entry carries no contents and is not a
//! file of the tree. A path is taken as the KEY of a file in the build's working tree —
//! `..` and a leading separator are dropped, so no entry of a caller's archive names
//! anything outside that tree, and nothing here opens a host path.
//!
//! Entries are read as text, which is what the build's working tree holds
//! ([`super::build_shell`]): bytes that are not UTF-8 are replaced the way a lossy read
//! replaces them, so an archive holding a binary entry still unpacks with that entry
//! present rather than failing the build.

use std::io::{Read, Write};

/// How many entries of one archive are unpacked, or written into one.
const MAX_ENTRIES: usize = 256;

/// How many bytes ONE entry may hold once expanded.
const MAX_ENTRY_BYTES: usize = 256 * 1024;

/// How many bytes the whole archive may expand to.
const MAX_EXPANDED_BYTES: usize = 4 * 1024 * 1024;

/// The files `archive` holds, as `(path, contents)` in the archive's own order — `None`
/// when those bytes are not a ZIP archive this crate reads at all.
///
/// An empty archive is `Some(Vec::new())`: it IS an archive, and it holds no file.
pub fn unpacked(archive: &[u8]) -> Option<Vec<(String, String)>> {
    let mut read = ::zip::ZipArchive::new(::std::io::Cursor::new(archive)).ok()?;
    let mut files: Vec<(String, String)> = Vec::new();
    let mut expanded: usize = 0;
    for index in 0..read.len().min(MAX_ENTRIES) {
        let Ok(mut entry) = read.by_index(index) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let Ok(name) = entry.name() else {
            continue;
        };
        let Some(path) = tree_path(&name) else {
            continue;
        };
        let declared = usize::try_from(entry.size()).unwrap_or(usize::MAX);
        if declared > MAX_ENTRY_BYTES || expanded.saturating_add(declared) > MAX_EXPANDED_BYTES {
            ::tracing::debug!(
                %path,
                declared,
                "an archive entry is larger than a build's working tree holds"
            );
            continue;
        }
        let mut bytes = Vec::new();
        // Bounded by what the entry declared plus one byte, so an entry whose directory
        // understates its contents is still not read past the bound above.
        if entry
            .by_ref()
            .take(u64::try_from(MAX_ENTRY_BYTES).unwrap_or(u64::MAX))
            .read_to_end(&mut bytes)
            .is_err()
        {
            continue;
        }
        expanded = expanded.saturating_add(bytes.len());
        files.push((path, String::from_utf8_lossy(&bytes).to_string()));
    }
    Some(files)
}

/// `files` as the bytes of a ZIP archive — `None` when the archive could not be written,
/// which is this process failing rather than anything the caller did.
///
/// At most [`MAX_ENTRIES`] entries are written, in the order given.
pub fn packed(files: &[(String, String)]) -> Option<Vec<u8>> {
    let mut buffer = ::std::io::Cursor::new(Vec::new());
    {
        let mut writer = ::zip::ZipWriter::new(&mut buffer);
        let options = ::zip::write::SimpleFileOptions::default();
        for (path, contents) in files.iter().take(MAX_ENTRIES) {
            writer.start_file(path.as_str(), options).ok()?;
            writer.write_all(contents.as_bytes()).ok()?;
        }
        writer.finish().ok()?;
    }
    Some(buffer.into_inner())
}

/// The path a build's working tree keys an archive entry by, or `None` for an entry that
/// names no file of it.
///
/// The tree's keys are the paths a buildspec's commands name, which are relative to the
/// build's own root: a leading separator and every `.` or `..` segment are dropped, so an
/// archive entry addresses a file under that root and nothing else.
fn tree_path(name: &str) -> Option<String> {
    let kept: Vec<&str> = name
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .collect();
    (!kept.is_empty()).then(|| kept.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bytes of an archive holding `entries`.
    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = ::std::io::Cursor::new(Vec::new());
        {
            let mut writer = ::zip::ZipWriter::new(&mut buffer);
            let options = ::zip::write::SimpleFileOptions::default();
            for (name, bytes) in entries {
                writer.start_file(*name, options).expect("a new entry");
                writer.write_all(bytes).expect("the entry's bytes");
            }
            writer.finish().expect("a finished archive");
        }
        buffer.into_inner()
    }

    /// RULE: an archive unpacks into the files of a build's working tree, keyed by the path
    /// the entry names under the build's own root — and bytes that are not an archive
    /// unpack into nothing at all.
    ///
    /// The rows differ along what the rule generalises over: how many entries, how the
    /// entry's path is spelled, and whether the bytes are an archive.
    #[test]
    fn an_archive_unpacks_into_the_files_of_a_working_tree() {
        /// One row: the case, the bytes, and the files they unpack into.
        type Row = (
            &'static str,
            Vec<u8>,
            Option<Vec<(&'static str, &'static str)>>,
        );
        let rows: Vec<Row> = vec![
            ("no entries", archive(&[]), Some(vec![])),
            (
                "one entry",
                archive(&[("payload.txt", b"TOKEN")]),
                Some(vec![("payload.txt", "TOKEN")]),
            ),
            (
                "an entry under a directory",
                archive(&[("src/payload.txt", b"TOKEN")]),
                Some(vec![("src/payload.txt", "TOKEN")]),
            ),
            (
                "an absolute entry path, taken under the build's root",
                archive(&[("/payload.txt", b"TOKEN")]),
                Some(vec![("payload.txt", "TOKEN")]),
            ),
            (
                "an entry path climbing out of the tree",
                archive(&[("../../payload.txt", b"TOKEN")]),
                Some(vec![("payload.txt", "TOKEN")]),
            ),
            (
                "several entries",
                archive(&[("a.txt", b"A"), ("b/c.txt", b"C")]),
                Some(vec![("a.txt", "A"), ("b/c.txt", "C")]),
            ),
            ("bytes that are not an archive", b"not a zip".to_vec(), None),
        ];
        for (case, bytes, expected) in rows {
            let unpacked = unpacked(&bytes);
            let read: Option<Vec<(&str, &str)>> = unpacked.as_ref().map(|files| {
                files
                    .iter()
                    .map(|(path, contents)| (path.as_str(), contents.as_str()))
                    .collect()
            });
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: what is packed is what unpacks — the files go in and come back out with the
    /// same paths and the same bytes.
    #[test]
    fn what_is_packed_is_what_unpacks() {
        let files = vec![
            ("out.txt".to_string(), "first\nsecond\n".to_string()),
            ("dist/app.js".to_string(), String::new()),
        ];
        let packed = packed(&files).expect("the archive is written");
        let unpacked = unpacked(&packed).expect("the archive reads back");
        assert_eq!(
            unpacked, files,
            "expected the files that were packed, got {unpacked:?}"
        );
    }
}
