// SPDX-License-Identifier: CC0-1.0

//! The PROGRAM an instance's own user data gives its guest to run at boot.
//!
//! "The user data to make available to the instance" (`RunInstancesRequest$UserData`) is not
//! a value a client sets to read it back: an instance's user data is what its init system
//! RUNS when the guest comes up, and what a client then observes of it is the output on that
//! instance's console ([`super::instance_console_text`]). So the stored user data is read
//! here into the commands a boot performs, and the boot performs them
//! ([`super::guest_console`]).
//!
//! # The forms read
//!
//! A document says WHICH form it is itself — cloud-init reads the form off the document's own
//! header, never off the request that carried it — and each form has a CADENCE, the boots
//! cloud-init runs that form's program on, which is what decides whether a restarted guest
//! runs the program again:
//!
//! * a `#cloud-config` document — the YAML form cloud-init documents, read with
//!   `yaml-rust2`, the pure-Rust YAML 1.2 implementation the other services that read
//!   caller-supplied YAML use. Its `bootcmd` runs on EVERY boot and its `runcmd` on the
//!   first, which is the split cloud-init publishes for those two modules (`bootcmd` is run
//!   from the init stage of every boot, `runcmd` from the final stage once per instance), and
//!   its `write_files` names FILES the machine's storage holds — "write arbitrary files",
//!   written once per instance, with the directories above them made as needed;
//! * a text that starts with `#!` — a user-data SCRIPT, which cloud-init runs once per
//!   instance, so its lines are first-boot commands;
//! * a text that starts with `#cloud-boothook` — a BOOTHOOK, the other script form
//!   cloud-init documents, which it runs "very early in the boot process" and, unlike a
//!   user-data script, on EVERY boot, so its lines are every-boot commands;
//! * an ARCHIVE, which is no program of its own but a carrier of several: a MIME multipart
//!   message, read with `mailparse`, the same reading of a MIME message the services that read
//!   an RFC 5322 message take, and the `#cloud-config-archive` list of the same documents
//!   written as YAML. Each
//!   PART is read as the form its own `Content-Type` (or its `type`) declares — and, for a
//!   part that declares none this emulator knows a cadence for, as the form the part's own
//!   text declares — so the program a part carries runs at its own form's cadence beside
//!   the others;
//! * COMPRESSED bytes: cloud-init decompresses user data that arrives gzipped, so what the
//!   guest runs is the document inside, read as whatever form THAT declares;
//! * BASE64 text: cloud-init decodes base64-encoded user data before it reads the form, so a
//!   payload a client encoded itself — which the API's own encoding then wrapped a second
//!   time — gives the guest the document inside it rather than nothing
//!   ([`base64_document`]).
//!
//! The one form left unread is the `#include` family, which names documents to FETCH over
//! the network: this emulator makes no outbound request, so there is nothing to run. Such a
//! document — like user data that is not text at all — gives the guest no program, and the
//! guest's console then carries the boot itself and no user-data output, rather than output
//! invented for a program nothing read.
//!
//! # Bounds
//!
//! The user data a launch may store is already bounded by that operation
//! (`RunInstances`'s 16 KiB ceiling), and every step of reading it is bounded again here,
//! BEFORE the step it bounds does any work: how many bytes of document are read at all
//! ([`MAX_GUEST_CONFIG_BYTES`]), how deeply its text can make a parse nest
//! ([`MAX_GUEST_CONFIG_TEXT_NESTING`], a scan of the text that parses nothing), how many
//! nodes it may carry and whether it aliases at all ([`MAX_GUEST_CONFIG_NODES`], read off
//! the parser's own event stream, which copies nothing), how deeply the loaded document
//! nests ([`MAX_GUEST_CONFIG_DEPTH`]), and how many commands one boot performs
//! ([`MAX_GUEST_COMMANDS`]) and how long one of them may be ([`MAX_GUEST_COMMAND_BYTES`]) —
//! so a document cannot make a boot cost more than a fixed amount of work however it nests,
//! anchors or aliases. An ARCHIVE is bounded the same way before it is opened: how many bytes
//! of it are read at all ([`MAX_GUEST_DOCUMENT_BYTES`], which is also what a compressed document may
//! decompress to), how many parts are read out of it ([`MAX_GUEST_ARCHIVE_PARTS`]) and how deeply
//! an archive may carry another ([`MAX_GUEST_ARCHIVE_NESTING`], a precondition of EVERY archive
//! reader rather than of one path into them, and carried through a part so that no part resets
//! it) — which is what bounds the recursion reading an archive of archives costs.

use ::base64::Engine as _;

/// How many commands ONE boot performs out of the instance's user data, counting its
/// every-boot and its first-boot commands together. A document that names more is read up to
/// this many and no further.
pub const MAX_GUEST_COMMANDS: usize = 64;

/// How many bytes one of those commands may carry. A longer one is not performed at all.
pub const MAX_GUEST_COMMAND_BYTES: usize = 4 * 1024;

/// How deeply the `#cloud-config` document may nest before it is not read at all, which is
/// what keeps the YAML reader's own recursion in range.
pub const MAX_GUEST_CONFIG_DEPTH: usize = 16;

/// How many nodes the TEXT of the document may nest by, counted before anything parses it.
///
/// The shared pre-parse scan counts every nesting indicator the text carries PLUS its
/// deepest indentation, so a sequence of entries counts one per entry rather than one for
/// the sequence: the ceiling therefore sits above what a document of at most
/// [`MAX_GUEST_COMMANDS`] entries needs, and it is still a bound on the recursion a parse
/// can reach, because every real level of nesting costs at least one of those. The
/// DOCUMENT's own depth is bounded exactly, by [`MAX_GUEST_CONFIG_DEPTH`].
pub const MAX_GUEST_CONFIG_TEXT_NESTING: usize = 1_024;

/// How many bytes of `#cloud-config` document are read at all.
///
/// The launch already bounds the user data it stores, and this bounds what reaches a parser
/// independently of that: the two are different operations' ceilings, and a parse must not
/// depend on another one having been applied.
pub const MAX_GUEST_CONFIG_BYTES: usize = 64 * 1024;

/// How many NODES the document may carry — scalars, sequences and mappings — counted on the
/// parser's own event stream before the document is loaded.
///
/// A byte ceiling does not bound the work a load performs: a loader resolves an ALIAS by
/// copying the anchored node, so a short document can expand enormously while it is built.
/// Counting the events the parser emits bounds that before anything is copied, and an alias
/// is refused outright beside it ([`within_load_budget`]).
pub const MAX_GUEST_CONFIG_NODES: usize = 4_096;

/// The header a `#cloud-config` user data starts with, as cloud-init documents it.
pub const CLOUD_CONFIG_HEADER: &str = "#cloud-config";

/// The header a BOOTHOOK user data starts with, as cloud-init documents it. Its script runs
/// on EVERY boot, which is what separates it from the `#!` script form.
pub const CLOUD_BOOTHOOK_HEADER: &str = "#cloud-boothook";

/// The header a `#cloud-config-archive` user data starts with: an ARCHIVE, a YAML list of the
/// documents it carries rather than a program of its own.
pub const CLOUD_CONFIG_ARCHIVE_HEADER: &str = "#cloud-config-archive";

/// The keys of one entry of that list: the `type` the entry is to be read as — the same media
/// type a MIME part declares — and the `content` it holds.
const ARCHIVE_ENTRY_TYPE: &str = "type";
const ARCHIVE_ENTRY_CONTENT: &str = "content";

/// The media types a part of an archive declares its form with, as cloud-init documents them:
/// the cloud-config and boothook forms, and the script form in each of the cadences it is
/// published under — once per instance (its default), and once per boot.
const PART_CLOUD_CONFIG: &str = "text/cloud-config";
const PART_CLOUD_CONFIG_ARCHIVE: &str = "text/cloud-config-archive";
const PART_CLOUD_BOOTHOOK: &str = "text/cloud-boothook";
const PART_SHELLSCRIPT: &str = "text/x-shellscript";
const PART_SHELLSCRIPT_PER_BOOT: &str = "text/x-shellscript-per-boot";
const PART_SHELLSCRIPT_PER_INSTANCE: &str = "text/x-shellscript-per-instance";
const PART_SHELLSCRIPT_PER_ONCE: &str = "text/x-shellscript-per-once";

/// The media type family an ARCHIVE declares itself under, as a part's own `Content-Type` and
/// as a message's: "multipart/mixed" is the one cloud-init documents, and every subtype of the
/// family carries parts the same way.
const MULTIPART_MEDIA_TYPE: &str = "multipart/";

/// The first two bytes of a gzip member (RFC 1952), which is what says a user data arrived
/// COMPRESSED.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// How many bytes of user data are read at all — before it is opened as an archive, and after
/// a compressed one is decompressed, so a short document cannot expand past it.
pub const MAX_GUEST_DOCUMENT_BYTES: usize = 64 * 1024;

/// How many parts are read out of ONE archive.
pub const MAX_GUEST_ARCHIVE_PARTS: usize = 16;

/// How deeply an archive may carry another archive — or a compressed document another — before
/// it is read no further.
///
/// Tested by EVERY reader that opens an archive ([`mime_archive_program`],
/// [`config_archive_program`]) and by [`document_program`], rather than on one path into them,
/// and counted once per archive OPENED: a part that carries a document rather than an archive
/// hands the count on unchanged, so no part is read under a wider budget than its own archive
/// was.
pub const MAX_GUEST_ARCHIVE_NESTING: usize = 4;

/// How many documents may DECLARE themselves to be archives inside one MIME message, counted
/// on its text before anything parses it.
///
/// The MIME reader is handed the whole message and builds the part tree itself, which is work
/// done before any ceiling of this module is consulted — so the declarations are counted first,
/// the way the YAML reader's nesting is bounded before its load. The ceiling is what a message
/// inside every other bound can even carry: one declaration per part of every level that may be
/// read.
pub const MAX_GUEST_ARCHIVE_DECLARATIONS: usize =
    MAX_GUEST_ARCHIVE_PARTS * (MAX_GUEST_ARCHIVE_NESTING + 1);

/// The key of the cloud-config module whose commands run on EVERY boot.
pub const CLOUD_CONFIG_BOOTCMD: &str = "bootcmd";

/// The key of the cloud-config module whose commands run on the FIRST boot only.
pub const CLOUD_CONFIG_RUNCMD: &str = "runcmd";

/// The key of the cloud-config module that WRITES FILES on the machine's storage, and the
/// keys of one of its entries: "write arbitrary files" at `path`, holding `content`, which
/// `encoding` says how to read and `append` says whether to add to what the path holds.
pub const CLOUD_CONFIG_WRITE_FILES: &str = "write_files";
const WRITE_FILES_PATH: &str = "path";
const WRITE_FILES_CONTENT: &str = "content";
const WRITE_FILES_ENCODING: &str = "encoding";
const WRITE_FILES_APPEND: &str = "append";

/// The `encoding` values that say the content is base64, as cloud-init names them. Every
/// other value leaves the content as the text it is, which is that module's own default.
const BASE64_ENCODINGS: &[&str] = &["b64", "base64"];

/// How many files ONE document may name, and how many bytes one of them may carry: the
/// bounds on what reading a caller-supplied document can put on a machine, taken before
/// anything is stored. The machine's storage is bounded again where it is built
/// ([`super::guest_console::MAX_GUEST_FILES`]).
pub const MAX_GUEST_WRITTEN_FILES: usize = 16;
pub const MAX_GUEST_WRITTEN_FILE_BYTES: usize = 32 * 1024;

/// One file a `write_files` module names: where it goes, what it holds, and whether it is
/// ADDED to what the path already holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestWrittenFile {
    pub path: String,
    pub content: String,
    pub append: bool,
}

/// The commands one instance's guest runs at boot, read off its user data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuestProgram {
    /// The commands the init system runs on EVERY boot — a cloud-config `bootcmd`.
    pub every_boot: Vec<String>,
    /// The commands it runs on the FIRST boot of the instance only — a cloud-config
    /// `runcmd`, or the lines of a user-data script.
    pub first_boot: Vec<String>,
    /// The files the document names on the machine's storage — a cloud-config `write_files`,
    /// which is a MODULE of the document rather than a command: the files are part of the
    /// storage the first boot of this instance starts with.
    pub written_files: Vec<GuestWrittenFile>,
}

impl GuestProgram {
    /// Whether this user data gives the guest nothing to run.
    pub fn is_empty(&self) -> bool {
        self.every_boot.is_empty() && self.first_boot.is_empty() && self.written_files.is_empty()
    }

    /// The commands boot number `boot` performs, in the order the init system runs them:
    /// the every-boot commands, and the first-boot ones only when this is the first boot.
    pub fn commands_for_boot(&self, boot: u32) -> Vec<&str> {
        let first_boot = if boot <= 1 {
            self.first_boot.as_slice()
        } else {
            &[]
        };
        self.every_boot
            .iter()
            .chain(first_boot)
            .map(String::as_str)
            .take(MAX_GUEST_COMMANDS)
            .collect()
    }
}

/// The program `user_data` gives the guest — empty for user data this emulator runs nothing
/// for (see the module docs).
///
/// `user_data` is the base64 text the launch stored ("the user data to make available to the
/// instance", which the API carries base64-encoded), so it is decoded here before it is
/// read.
pub fn guest_program(user_data: Option<&str>) -> GuestProgram {
    let Some(bytes) = user_data.and_then(base64_document) else {
        return GuestProgram::default();
    };
    document_program(&bytes, 0)
}

/// The program the document `bytes` carries, read `nesting` archives deep.
///
/// Every bound a reading is given is taken HERE, before the reading: the bytes read at all,
/// and how deeply an archive or a compressed document may carry another.
fn document_program(bytes: &[u8], nesting: usize) -> GuestProgram {
    if nesting > MAX_GUEST_ARCHIVE_NESTING || bytes.len() > MAX_GUEST_DOCUMENT_BYTES {
        return GuestProgram::default();
    }
    // cloud-init decompresses user data that arrives compressed, so what the guest runs is
    // the document INSIDE, read as whatever form that one declares itself to be.
    if let Some(decompressed) = decompressed(bytes) {
        return document_program(&decompressed, nesting + 1);
    }
    let Ok(text) = ::std::str::from_utf8(bytes) else {
        return GuestProgram::default();
    };
    text_program(text, nesting)
}

/// The program the document `text` carries, read as the form its own header declares.
fn text_program(text: &str, nesting: usize) -> GuestProgram {
    let trimmed = text.trim_start();
    // An ARCHIVE carries documents rather than commands, so it is recognised before any
    // program form; the archive header starts with the cloud-config one, so it is read first.
    if is_mime_archive(trimmed) {
        return mime_archive_program(trimmed, nesting + 1);
    }
    if trimmed.starts_with(CLOUD_CONFIG_ARCHIVE_HEADER) {
        return config_archive_program(trimmed, nesting + 1);
    }
    if trimmed.starts_with(CLOUD_CONFIG_HEADER) {
        return cloud_config_program(trimmed);
    }
    if trimmed.starts_with(CLOUD_BOOTHOOK_HEADER) {
        // A boothook is run on EVERY boot, so its lines are every-boot commands — the one
        // way a user-data SCRIPT (rather than a cloud-config module) runs again on a guest
        // that restarted.
        return every_boot_program(trimmed);
    }
    if trimmed.starts_with("#!") {
        // A user-data script is run ONCE per instance, so its lines are the first boot's.
        return first_boot_program(trimmed);
    }
    // A text that declares no form of its own may still CARRY a document: cloud-init accepts
    // base64-encoded user data and decodes it before it reads the form, exactly as it
    // decompresses a gzipped one, so a payload a client encoded itself — which the API
    // encoding then wrapped a second time — arrives here as the base64 text of the real
    // document. What the guest runs is that document, read as whatever form IT declares.
    if let Some(carried) = base64_document(trimmed) {
        let program = document_program(&carried, nesting + 1);
        if !program.is_empty() {
            return program;
        }
    }
    GuestProgram::default()
}

/// The document the base64 text `text` stands for, or `None` when it is not base64 text at
/// all.
///
/// The ONE reading of "this text is base64 of a document" in this module, taken both of the
/// value the API carries ("the user data to make available to the instance", which travels
/// base64-encoded) and of a document found INSIDE it. Held to the same ceiling every other
/// reading of a carried document is ([`MAX_GUEST_DOCUMENT_BYTES`], tested on the text before
/// it is decoded, since base64 is longer than what it stands for) and to the shortest text
/// that can encode anything, so a stray word is not decoded as a document.
fn base64_document(text: &str) -> Option<Vec<u8>> {
    let packed: String = text.chars().filter(|value| !value.is_whitespace()).collect();
    if packed.len() < 4 || packed.len() > MAX_GUEST_DOCUMENT_BYTES {
        return None;
    }
    ::base64::engine::general_purpose::STANDARD
        .decode(packed.as_bytes())
        .ok()
        .filter(|carried| !carried.is_empty())
}

/// The script `text` as a program run on every boot.
fn every_boot_program(text: &str) -> GuestProgram {
    GuestProgram {
        every_boot: script_commands(text),
        first_boot: Vec::new(),
        written_files: Vec::new(),
    }
}

/// The script `text` as a program run on the first boot of the instance only.
fn first_boot_program(text: &str) -> GuestProgram {
    GuestProgram {
        every_boot: Vec::new(),
        first_boot: script_commands(text),
        written_files: Vec::new(),
    }
}

/// `bytes` decompressed, for user data that arrived as a gzip member — `None` for one that did
/// not, or one whose stream does not decompress to inside [`MAX_GUEST_DOCUMENT_BYTES`].
///
/// The output is bounded as it is read rather than after: a short member can expand without
/// limit, so the reader is handed the ceiling plus one byte and a stream that reaches it is
/// read no further.
fn decompressed(bytes: &[u8]) -> Option<Vec<u8>> {
    use ::std::io::Read as _;

    if !bytes.starts_with(&GZIP_MAGIC) {
        return None;
    }
    let mut decompressed = Vec::new();
    ::flate2::read::GzDecoder::new(bytes)
        .take(MAX_GUEST_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut decompressed)
        .ok()?;
    (decompressed.len() <= MAX_GUEST_DOCUMENT_BYTES).then_some(decompressed)
}

/// Whether `line` is a `Content-Type` header declaring a MULTIPART media type — the one
/// reading of such a line, shared by the scan that tells an archive from a document and the
/// count that bounds what one message declares.
fn declares_multipart(line: &str) -> bool {
    let line = line.trim().to_ascii_lowercase();
    line.starts_with("content-type:") && line.contains(MULTIPART_MEDIA_TYPE)
}

/// Whether `text` is a MIME multipart archive: its header block declares one, which is how
/// cloud-init tells an archive from a document.
fn is_mime_archive(text: &str) -> bool {
    text.lines()
        .take_while(|line| !line.trim().is_empty())
        .any(declares_multipart)
}

/// The program a MIME multipart archive carries: each part read as the form its own
/// `Content-Type` declares, in the order the archive carries them.
///
/// The header block and the part bodies are read through `mailparse`, the same reading of a
/// MIME message the services that read an RFC 5322 message take — which also decodes a part's
/// own transfer encoding, so a base64 part is the bytes it stands for.
fn mime_archive_program(text: &str, nesting: usize) -> GuestProgram {
    // The bound on what the READER builds: it is handed the whole message and builds the part
    // tree before any ceiling here is consulted, so what the text DECLARES is counted before it
    // is parsed — the same order the YAML reader's nesting is bounded in. (The nesting bound
    // itself is [`mime_parts_program`]'s, so it holds for a nested archive too.)
    if multipart_declarations(text) > MAX_GUEST_ARCHIVE_DECLARATIONS {
        return GuestProgram::default();
    }
    let Ok(message) = ::mailparse::parse_mail(text.as_bytes()) else {
        return GuestProgram::default();
    };
    mime_parts_program(&message.subparts, nesting)
}

/// The program the PARTS of one MIME archive carry, each read as the form its own
/// `Content-Type` declares — and a part that declares itself an ARCHIVE read as the parts the
/// reader has already built for it, one level deeper.
fn mime_parts_program(parts: &[::mailparse::ParsedMail<'_>], nesting: usize) -> GuestProgram {
    // The bound on an archive of archives, taken by every reader that opens one.
    if nesting > MAX_GUEST_ARCHIVE_NESTING {
        return GuestProgram::default();
    }
    let mut program = GuestProgram::default();
    for part in parts.iter().take(MAX_GUEST_ARCHIVE_PARTS) {
        let declared = part.ctype.mimetype.trim().to_ascii_lowercase();
        // A part whose own media type is a multipart one carries PARTS rather than a document,
        // and the reader built them when it read the message: they are this archive's parts one
        // level down.
        if declared.starts_with(MULTIPART_MEDIA_TYPE) {
            add(&mut program, mime_parts_program(&part.subparts, nesting + 1));
            continue;
        }
        let Ok(body) = part.get_body_raw() else {
            continue;
        };
        add(&mut program, part_program(&declared, &body, nesting));
    }
    program
}

/// How many documents `text` declares to be archives: one `Content-Type` naming a multipart
/// media type per level and per part that carries one. Counted on the text, parsing nothing.
fn multipart_declarations(text: &str) -> usize {
    text.lines().filter(|line| declares_multipart(line)).count()
}

/// The program a `#cloud-config-archive` carries: the same parts written as a YAML list, each
/// entry naming the `type` it is to be read as and the `content` it holds — an entry that is
/// one plain text being the document its own header declares.
fn config_archive_program(text: &str, nesting: usize) -> GuestProgram {
    // The bound on an archive of archives, taken by every reader that opens one: this form is
    // reached both from a document's own header and from a part that declares it, so the
    // precondition is here rather than on one of those paths.
    if nesting > MAX_GUEST_ARCHIVE_NESTING {
        return GuestProgram::default();
    }
    let Some(document) = loaded_document(text) else {
        return GuestProgram::default();
    };
    let Some(entries) = document.as_vec() else {
        return GuestProgram::default();
    };
    let mut program = GuestProgram::default();
    for entry in entries.iter().take(MAX_GUEST_ARCHIVE_PARTS) {
        let Some(content) = entry[ARCHIVE_ENTRY_CONTENT]
            .as_str()
            .or_else(|| entry.as_str())
        else {
            continue;
        };
        let declared = entry[ARCHIVE_ENTRY_TYPE].as_str().unwrap_or_default();
        add(
            &mut program,
            part_program(declared, content.as_bytes(), nesting),
        );
    }
    program
}

/// The program ONE part of an archive carries, read as the form the media type `declared`
/// names — and, for a part declaring a type no cadence is published for, as the form the
/// part's own document declares.
///
/// `nesting` is the count the archive this part came out of was read under: handed on unchanged
/// for a part carrying a DOCUMENT and raised for one carrying an ARCHIVE, since the count is
/// one per archive opened and every reader that opens one tests it — so a part is never read
/// under a wider budget than its own archive was.
fn part_program(declared: &str, body: &[u8], nesting: usize) -> GuestProgram {
    let declared = declared.trim().to_ascii_lowercase();
    // A part's own bytes may be COMPRESSED, as a whole user data may be, and a part is then
    // the document inside them: that reading is [`document_program`]'s, so a part that is not
    // text goes there rather than carrying no program.
    let Ok(text) = ::std::str::from_utf8(body) else {
        return document_program(body, nesting);
    };
    match declared.as_str() {
        PART_CLOUD_CONFIG => cloud_config_program(text.trim_start()),
        PART_CLOUD_CONFIG_ARCHIVE => config_archive_program(text.trim_start(), nesting + 1),
        // The boothook and the per-boot script form are the two run on every boot.
        PART_CLOUD_BOOTHOOK | PART_SHELLSCRIPT_PER_BOOT => every_boot_program(text),
        PART_SHELLSCRIPT | PART_SHELLSCRIPT_PER_INSTANCE | PART_SHELLSCRIPT_PER_ONCE => {
            first_boot_program(text)
        }
        _ => document_program(body, nesting),
    }
}

/// Add what one part gives the guest to `program`, holding the whole to the bounds ONE
/// document is held to: an archive of many parts cannot make a boot perform more than a
/// single document could.
fn add(program: &mut GuestProgram, part: GuestProgram) {
    program.every_boot.extend(part.every_boot);
    program.first_boot.extend(part.first_boot);
    program.written_files.extend(part.written_files);
    program.every_boot.truncate(MAX_GUEST_COMMANDS);
    program.first_boot.truncate(MAX_GUEST_COMMANDS);
    program.written_files.truncate(MAX_GUEST_WRITTEN_FILES);
}

/// The commands a `#cloud-config` document names, from its `bootcmd` and its `runcmd`.
fn cloud_config_program(text: &str) -> GuestProgram {
    let Some(document) = loaded_document(text) else {
        return GuestProgram::default();
    };
    GuestProgram {
        every_boot: module_commands(&document, CLOUD_CONFIG_BOOTCMD),
        first_boot: module_commands(&document, CLOUD_CONFIG_RUNCMD),
        written_files: module_files(&document),
    }
}

/// `text` loaded as the YAML document it is — `None` for one outside any bound a load is
/// given, which is a document nothing is read out of.
///
/// Nothing is loaded until the text is known to be inside every one of those bounds: its
/// size, how deeply it nests (the shared pre-parse scan, which parses nothing) and what its
/// own event stream reports ([`within_load_budget`]). The ONE load in this module, shared by
/// the cloud-config form and the `#cloud-config-archive` list of documents.
fn loaded_document(text: &str) -> Option<::yaml_rust2::Yaml> {
    if text.len() > MAX_GUEST_CONFIG_BYTES {
        return None;
    }
    // The ONE pre-parse nesting bound this workspace states, taken on the TEXT: a parser
    // costs a level of recursion per nested collection, so the depth is bounded before the
    // parse rather than observed during it.
    if ::service_common::yaml_nesting::nesting_depth_bound(text, MAX_GUEST_CONFIG_TEXT_NESTING)
        > MAX_GUEST_CONFIG_TEXT_NESTING
    {
        return None;
    }
    if !within_load_budget(text) {
        return None;
    }
    let document = ::yaml_rust2::YamlLoader::load_from_str(text)
        .ok()?
        .into_iter()
        .next()?;
    within_depth(&document, MAX_GUEST_CONFIG_DEPTH).then_some(document)
}

/// The files the `write_files` module names, each as the path it goes to and the content it
/// holds — read up to [`MAX_GUEST_WRITTEN_FILES`] entries, skipping an entry that names no
/// path, one whose content this emulator cannot read (an encoding it does not decode, or
/// base64 that does not decode to text) and one past [`MAX_GUEST_WRITTEN_FILE_BYTES`].
fn module_files(document: &::yaml_rust2::Yaml) -> Vec<GuestWrittenFile> {
    let Some(entries) = document[CLOUD_CONFIG_WRITE_FILES].as_vec() else {
        return Vec::new();
    };
    let mut files: Vec<GuestWrittenFile> = Vec::new();
    for entry in entries {
        if files.len() >= MAX_GUEST_WRITTEN_FILES {
            break;
        }
        let Some(path) = entry[WRITE_FILES_PATH].as_str() else {
            continue;
        };
        if path.trim().is_empty() {
            continue;
        }
        // "content: the content to write", left out for an entry that names none, which
        // cloud-init writes as an empty file.
        let written = entry[WRITE_FILES_CONTENT].as_str().unwrap_or_default();
        let encoding = entry[WRITE_FILES_ENCODING].as_str().unwrap_or_default();
        let content = if BASE64_ENCODINGS.contains(&encoding.trim()) {
            let Some(decoded) = decoded_base64_text(written) else {
                continue;
            };
            decoded
        } else if encoding.trim().is_empty() {
            written.to_string()
        } else {
            // An encoding this emulator does not decode (a compressed one) names content it
            // cannot read, so no file is written rather than one holding the undecoded text.
            continue;
        };
        if content.len() > MAX_GUEST_WRITTEN_FILE_BYTES {
            continue;
        }
        files.push(GuestWrittenFile {
            path: path.trim().to_string(),
            content,
            // "append: whether to append to the file", which is `false` unless the entry
            // says otherwise.
            append: entry[WRITE_FILES_APPEND].as_bool().unwrap_or(false),
        });
    }
    files
}

/// `text` as the base64 an entry's own `encoding` declares it to be — `None` when it is not
/// base64 at all, or not UTF-8 text once decoded.
fn decoded_base64_text(text: &str) -> Option<String> {
    let decoded = ::base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .ok()?;
    String::from_utf8(decoded).ok()
}

/// Whether `text` is inside the budget a LOAD of it is given, read off the parser's own
/// event stream — which reports what the document holds and copies nothing.
///
/// Two things are bounded here that a byte ceiling does not bound. An ALIAS is refused
/// outright: a loader resolves one by copying the anchored node into each referencing
/// position, so a document that nests shallowly and weighs little can still expand
/// multiplicatively while it is built. And the NODES the stream reports are counted, so the
/// work the load would do is known to be bounded before any of it is done.
///
/// A document the parser cannot read at all is not inside the budget either: there is nothing
/// to load.
fn within_load_budget(text: &str) -> bool {
    /// What the event stream reported: how many nodes, how deeply, and whether an alias was
    /// among them.
    #[derive(Default)]
    struct Structure {
        nodes: usize,
        depth: usize,
        deepest: usize,
        saw_alias: bool,
    }
    impl ::yaml_rust2::parser::EventReceiver for Structure {
        fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
            match event {
                ::yaml_rust2::parser::Event::SequenceStart(..)
                | ::yaml_rust2::parser::Event::MappingStart(..) => {
                    self.nodes = self.nodes.saturating_add(1);
                    self.depth = self.depth.saturating_add(1);
                    self.deepest = self.deepest.max(self.depth);
                }
                ::yaml_rust2::parser::Event::SequenceEnd
                | ::yaml_rust2::parser::Event::MappingEnd => {
                    self.depth = self.depth.saturating_sub(1);
                }
                ::yaml_rust2::parser::Event::Scalar(..) => {
                    self.nodes = self.nodes.saturating_add(1);
                }
                ::yaml_rust2::parser::Event::Alias(_) => self.saw_alias = true,
                _ => {}
            }
        }
    }

    let mut structure = Structure::default();
    if ::yaml_rust2::parser::Parser::new_from_str(text)
        .load(&mut structure, true)
        .is_err()
    {
        return false;
    }
    !structure.saw_alias
        && structure.nodes <= MAX_GUEST_CONFIG_NODES
        && structure.deepest <= MAX_GUEST_CONFIG_DEPTH
}

/// The commands the cloud-config module `key` names: each entry of its list, as one command
/// line per line of that entry.
///
/// An entry cloud-init documents as a LIST ("`[ cloud-init-per, once, mymkfs, mkfs, /dev/vdb
/// ]`") is the argument vector of one command, which cloud-init runs WITHOUT a shell — so
/// its words are quoted back into the one command line that stands for that vector, and a
/// word of it carrying a space or an operator is still one word; an entry written as text is
/// the text itself, which may carry several lines.
fn module_commands(document: &::yaml_rust2::Yaml, key: &str) -> Vec<String> {
    let Some(entries) = document[key].as_vec() else {
        // A module written as one scalar is one entry, which is the other form cloud-init
        // accepts for these keys.
        return document[key]
            .as_str()
            .map(script_commands)
            .unwrap_or_default();
    };
    let mut commands: Vec<String> = Vec::new();
    for entry in entries {
        if commands.len() >= MAX_GUEST_COMMANDS {
            break;
        }
        match entry {
            ::yaml_rust2::Yaml::String(text) => commands.extend(script_commands(text)),
            ::yaml_rust2::Yaml::Array(words) => {
                let argv: Option<Vec<String>> = words
                    .iter()
                    .map(|word| match word {
                        ::yaml_rust2::Yaml::String(word) => Some(word.clone()),
                        _ => None,
                    })
                    .collect();
                if let Some(argv) = argv {
                    commands.extend(script_commands(&::shell_words::join(&argv)));
                }
            }
            _ => {}
        }
    }
    commands.truncate(MAX_GUEST_COMMANDS);
    commands
}

/// The commands of a script: its lines, without the ones a shell performs nothing for — the
/// blank ones and the comments, the interpreter line included.
///
/// A line carrying a HERE-DOCUMENT (`<<WORD`, `<<-WORD`) is one command TOGETHER with the
/// lines up to its delimiter: the document is part of the command that reads it, which is how
/// a shell reads one, so those lines are kept verbatim rather than read as commands of their
/// own.
fn script_commands(text: &str) -> Vec<String> {
    let mut commands: Vec<String> = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if commands.len() >= MAX_GUEST_COMMANDS {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut command = trimmed.to_string();
        if let Some(delimiter) = here_document_delimiter(trimmed) {
            // The document's own lines, as written: the delimiter line ends it, and a
            // document that never reaches one runs out with the script.
            for next in lines.by_ref() {
                command.push('\n');
                command.push_str(next);
                if next.trim() == delimiter || command.len() > MAX_GUEST_COMMAND_BYTES {
                    break;
                }
            }
        }
        if command.len() <= MAX_GUEST_COMMAND_BYTES {
            commands.push(command);
        }
    }
    commands
}

/// The DELIMITER the here-document `line` opens, or `None` for a line that opens none.
///
/// `<<WORD` and `<<-WORD` open one, with the word optionally quoted — the quoting decides
/// whether the document is expanded, which the shell itself reads ("a here-document whose
/// delimiter was quoted is taken literally"). `<<<WORD` is a here-STRING: its text is on the
/// line already, so it opens no document. A `<<` whose word is not a plain one, or which is
/// followed by text rather than by the operands a redirection may carry, opens none either:
/// that is a `<<` inside a quoted argument rather than a redirection.
fn here_document_delimiter(line: &str) -> Option<String> {
    let (_, rest) = line.split_once("<<")?;
    if rest.starts_with('<') {
        return None;
    }
    let rest = rest.strip_prefix('-').unwrap_or(rest).trim_start();
    let (quote, rest) = match rest.chars().next() {
        Some(quote @ ('\'' | '"')) => (Some(quote), rest.get(1..)?),
        _ => (None, rest),
    };
    let word: String = rest
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
        .collect();
    if word.is_empty() {
        return None;
    }
    let after = rest.get(word.len()..)?;
    let after = match quote {
        Some(quote) => after.strip_prefix(quote)?,
        None => after,
    };
    // What may follow the delimiter is what may follow a redirection: nothing, or another
    // operand of the same command.
    let follows = after
        .chars()
        .next()
        .is_none_or(|character| character.is_whitespace() || "|&;<>".contains(character));
    follows.then_some(word)
}

/// Whether `node` nests inside `depth` levels — what keeps this walk, and the document it
/// walks, in range.
fn within_depth(node: &::yaml_rust2::Yaml, depth: usize) -> bool {
    let Some(depth) = depth.checked_sub(1) else {
        return false;
    };
    match node {
        ::yaml_rust2::Yaml::Array(entries) => {
            entries.iter().all(|entry| within_depth(entry, depth))
        }
        ::yaml_rust2::Yaml::Hash(entries) => entries
            .iter()
            .all(|(_, value)| within_depth(value, depth)),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::{guest_program, GuestProgram, MAX_GUEST_COMMANDS};
    use ::base64::Engine as _;

    /// `text` as a launch stores user data: base64.
    fn encoded(text: &str) -> String {
        ::base64::engine::general_purpose::STANDARD.encode(text)
    }

    /// The same, for user data that is not text at all.
    fn encoded_bytes(bytes: &[u8]) -> String {
        ::base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// `text` as the gzip member a compressed user data arrives as.
    fn compressed(text: &str) -> Vec<u8> {
        use ::std::io::Write as _;

        let mut encoder =
            ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
        encoder
            .write_all(text.as_bytes())
            .expect("a gzip member of the document");
        encoder.finish().expect("the finished gzip member")
    }

    /// The PARTS section of an archive bounded by `boundary`: the delimited parts, with no
    /// header block of its own — which is also exactly what the body of a part that is itself an
    /// archive carries.
    fn mime_parts(boundary: &str, parts: &[(&str, &str)]) -> String {
        let mut text = String::new();
        for (declared, body) in parts {
            text.push_str(&format!("--{boundary}\n"));
            if !declared.is_empty() {
                text.push_str(&format!("Content-Type: {declared}\n"));
            }
            text.push('\n');
            text.push_str(body);
            text.push('\n');
        }
        text.push_str(&format!("--{boundary}--\n"));
        text
    }

    /// A MIME multipart archive carrying `parts`, each as `(the media type it declares, its
    /// body)` — the form a client assembles one in. `boundary` is the delimiter the archive
    /// separates its parts with, so an archive carrying another names its own.
    fn mime_archive_bounded_by(boundary: &str, parts: &[(&str, &str)]) -> String {
        format!(
            "Content-Type: multipart/mixed; boundary=\"{boundary}\"\nMIME-Version: 1.0\n\n{}",
            mime_parts(boundary, parts)
        )
    }

    /// The same archive under the one boundary a suite that nests nothing needs.
    fn mime_archive(parts: &[(&str, &str)]) -> String {
        mime_archive_bounded_by("==BOUNDARY==", parts)
    }

    /// A MIME archive nested `levels` deep below its top level, whose innermost part is a
    /// boothook running `command`.
    ///
    /// Each level is a PART that declares the multipart media type and a boundary of its own,
    /// carrying the next level's parts — which is how a client nests archives, and what makes
    /// the reader build the tree rather than this helper repeating a header block in a body.
    fn nested_mime_archive(levels: usize, command: &str) -> String {
        /// `(the boundary this level is delimited by, its parts section)`.
        fn level(at: usize, levels: usize, command: &str) -> (String, String) {
            let boundary = format!("==LEVEL-{at}==");
            if at >= levels {
                let parts = mime_parts(&boundary, &[("text/cloud-boothook", command)]);
                return (boundary, parts);
            }
            let (inner, carried) = level(at + 1, levels, command);
            let declared = format!("multipart/mixed; boundary=\"{inner}\"");
            let parts = mime_parts(&boundary, &[(declared.as_str(), carried.as_str())]);
            (boundary, parts)
        }

        let (boundary, parts) = level(0, levels, command);
        format!(
            "Content-Type: multipart/mixed; boundary=\"{boundary}\"\nMIME-Version: 1.0\n\n{parts}"
        )
    }

    /// RULE: a `#cloud-config` document's `bootcmd` runs on every boot and its `runcmd` on
    /// the first, a `#cloud-boothook` script runs on every boot, a `#!` script runs once, and
    /// every other form of user data gives the guest nothing to run.
    #[test]
    fn the_program_is_what_the_user_data_names() {
        // (case, the user data, its every-boot commands, its first-boot commands)
        let rows: [(&str, String, Vec<&str>, Vec<&str>); 11] = [
            (
                "a cloud-config with both modules",
                encoded(
                    "#cloud-config\nbootcmd:\n  - echo every > /dev/console\nruncmd:\n  - echo once > /dev/console\n",
                ),
                vec!["echo every > /dev/console"],
                vec!["echo once > /dev/console"],
            ),
            (
                "a cloud-config whose entry is a block of lines",
                encoded(
                    "#cloud-config\nbootcmd:\n  - |\n    C=1\n    echo $C > /dev/console\n",
                ),
                vec!["C=1", "echo $C > /dev/console"],
                vec![],
            ),
            (
                "a cloud-config whose entry is an argument vector",
                encoded("#cloud-config\nbootcmd:\n  - [ echo, marker ]\n"),
                vec!["echo marker"],
                vec![],
            ),            (
                "a cloud-config whose module is one scalar",
                encoded("#cloud-config\nruncmd: echo scalar\n"),
                vec![],
                vec!["echo scalar"],
            ),
            (
                "a cloud-config with comments and blank lines",
                encoded("#cloud-config\nbootcmd:\n  - |\n    # a comment\n\n    echo kept\n"),
                vec!["echo kept"],
                vec![],
            ),
            (
                "a user-data script",
                encoded("#!/bin/sh\necho from-the-script\n"),
                vec![],
                vec!["echo from-the-script"],
            ),
            (
                // A boothook is the OTHER script form, and cloud-init runs it on every boot.
                "a boothook, whose script runs on every boot",
                encoded("#cloud-boothook\n#!/bin/sh\necho from-the-boothook\n"),
                vec!["echo from-the-boothook"],
                vec![],
            ),
            (
                "a boothook that carries no interpreter line",
                encoded("#cloud-boothook\necho bare\necho again\n"),
                vec!["echo bare", "echo again"],
                vec![],
            ),
            (
                "a boothook whose header is not the first byte",
                encoded("\n#cloud-boothook\necho after-a-blank-line\n"),
                vec!["echo after-a-blank-line"],
                vec![],
            ),
            (
                "an include form this emulator runs nothing for",
                encoded("#include\nhttps://example.invalid/script\n"),
                vec![],
                vec![],
            ),
            ("user data that is not base64", "not base64!".to_string(), vec![], vec![]),
        ];
        for (case, user_data, every_boot, first_boot) in rows {
            let program = guest_program(Some(&user_data));
            assert_eq!(
                program.every_boot, every_boot,
                "case {case:?}: expected the every-boot commands {every_boot:?}, got {:?}",
                program.every_boot
            );
            assert_eq!(
                program.first_boot, first_boot,
                "case {case:?}: expected the first-boot commands {first_boot:?}, got {:?}",
                program.first_boot
            );
        }
        assert!(
            guest_program(None).is_empty(),
            "an instance with no user data must have no program"
        );
    }

    /// RULE: a line carrying a here-document is ONE command with the lines up to its
    /// delimiter, and a `<<` that opens no document leaves the lines after it as commands of
    /// their own.
    #[test]
    fn a_here_document_is_part_of_the_command_that_reads_it() {
        // (case, the script, the commands it gives the guest)
        let rows: &[(&str, &str, &[&str])] = &[
            (
                "a quoted delimiter",
                "#!/bin/sh\ncat > /page <<'EOF'\nMARKER\nEOF\necho after\n",
                &["cat > /page <<'EOF'\nMARKER\nEOF", "echo after"],
            ),
            (
                "the document before another operand",
                "#!/bin/sh\ncat <<EOF > /page\nMARKER\nEOF\n",
                &["cat <<EOF > /page\nMARKER\nEOF"],
            ),
            (
                "the tab-stripping form",
                "#!/bin/sh\ncat > /page <<-EOF\n\tMARKER\nEOF\n",
                &["cat > /page <<-EOF\n\tMARKER\nEOF"],
            ),
            (
                "a here-string, which opens no document",
                "#!/bin/sh\ncat > /page <<<'one word'\necho after\n",
                &["cat > /page <<<'one word'", "echo after"],
            ),
            (
                "a `<<` inside a quoted argument",
                "#!/bin/sh\necho \"a << b\"\necho after\n",
                &["echo \"a << b\"", "echo after"],
            ),
        ];
        for (case, script, expected) in rows {
            let program = guest_program(Some(&encoded(script)));
            assert_eq!(
                program.first_boot, *expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                program.first_boot
            );
        }
    }

    /// RULE: a `write_files` module names FILES the machine's storage holds, read as the
    /// text they carry or as the base64 an entry's own `encoding` declares — and an entry
    /// this emulator cannot read names no file at all.
    #[test]
    fn the_write_files_module_names_the_files_the_storage_holds() {
        let document = concat!(
            "#cloud-config\n",
            "write_files:\n",
            "  - path: /var/www/html/index.html\n",
            "    content: |\n",
            "      <h1>served</h1>\n",
            "  - path: /etc/motd\n",
            "    encoding: b64\n",
            "    content: ZnJvbS1iYXNlNjQ=\n",
            "  - path: /var/log/added\n",
            "    content: one more line\n",
            "    append: true\n",
            "  - path: /etc/compressed\n",
            "    encoding: gzip+base64\n",
            "    content: H4sIAAAAAAAA\n",
            "  - content: no path at all\n",
        );
        let program = guest_program(Some(&encoded(document)));
        let written: Vec<(&str, &str, bool)> = program
            .written_files
            .iter()
            .map(|file| (file.path.as_str(), file.content.as_str(), file.append))
            .collect();
        assert_eq!(
            written,
            [
                ("/var/www/html/index.html", "<h1>served</h1>\n", false),
                ("/etc/motd", "from-base64", false),
                ("/var/log/added", "one more line", true),
            ],
            "expected the files the module names, got {written:?}"
        );
    }

    /// RULE: which commands a boot performs is the boot NUMBER's question — the every-boot
    /// commands always, the first-boot ones only on the first.
    #[test]
    fn a_later_boot_performs_only_the_every_boot_commands() {
        let program = GuestProgram {
            every_boot: vec!["echo every".to_string()],
            first_boot: vec!["echo once".to_string()],
            ..GuestProgram::default()
        };
        // (the boot, the commands it performs)
        for (boot, expected) in [
            (1_u32, vec!["echo every", "echo once"]),
            (2, vec!["echo every"]),
            (7, vec!["echo every"]),
        ] {
            let commands = program.commands_for_boot(boot);
            assert_eq!(
                commands, expected,
                "case boot {boot}: expected {expected:?}, got {commands:?}"
            );
        }
    }

    /// RULE: what is read out of one document is bounded, so a document cannot make a boot
    /// perform more than a fixed number of commands.
    #[test]
    fn the_commands_read_out_of_one_document_are_bounded() {
        let many: String = (0..MAX_GUEST_COMMANDS * 4)
            .map(|index| format!("  - echo line-{index}\n"))
            .collect();
        let program = guest_program(Some(&encoded(&format!("#cloud-config\nbootcmd:\n{many}"))));
        assert_eq!(
            program.every_boot.len(),
            MAX_GUEST_COMMANDS,
            "expected the commands to stop at the bound, got {}",
            program.every_boot.len()
        );
        let long = format!("echo {}", "x".repeat(super::MAX_GUEST_COMMAND_BYTES));
        let program = guest_program(Some(&encoded(&format!(
            "#cloud-config\nbootcmd:\n  - {long}\n"
        ))));
        assert!(
            program.is_empty(),
            "expected a command past the size bound to be left out, got {:?}",
            program.every_boot
        );
    }

    /// RULE: a document is bounded BEFORE it is loaded — its size, how deeply its text can
    /// make a parse nest, how many nodes it carries and whether it aliases at all — so one
    /// that would cost the load more than a fixed amount of work runs nothing rather than
    /// being built first.
    #[test]
    fn a_document_outside_the_load_budget_runs_nothing() {
        // (case, the document)
        let rows = [
            (
                "past the byte ceiling",
                format!(
                    "#cloud-config\nbootcmd:\n  - echo {}\n",
                    "x".repeat(super::MAX_GUEST_CONFIG_BYTES)
                ),
            ),
            (
                "past the nesting ceiling",
                format!(
                    "#cloud-config\nbootcmd:\n  - echo nested\nnested: {}\n",
                    "[".repeat(super::MAX_GUEST_CONFIG_TEXT_NESTING + 2)
                ),
            ),
            (
                "past the node ceiling",
                format!(
                    "#cloud-config\nbootcmd:\n  - echo many\nmany: [{}]\n",
                    (0..super::MAX_GUEST_CONFIG_NODES + 2)
                        .map(|index| format!("node-{index},"))
                        .collect::<String>()
                ),
            ),
            (
                // A loader resolves an alias by COPYING the anchored node, so a document
                // that aliases is refused outright rather than expanded.
                "an alias, which a load would copy",
                "#cloud-config\nanchored: &anchor [a, b, c]\nbootcmd:\n  - echo aliased\naliased: *anchor\n"
                    .to_string(),
            ),
        ];
        for (case, document) in rows {
            let program = guest_program(Some(&encoded(&document)));
            assert!(
                program.is_empty(),
                "case {case:?}: expected a document outside the load budget to run nothing, \
                 got {program:?}"
            );
        }

        // The same document inside every one of those bounds is read.
        let program = guest_program(Some(&encoded(
            "#cloud-config\nbootcmd:\n  - echo inside the budget\n",
        )));
        assert_eq!(
            program.every_boot,
            ["echo inside the budget"],
            "expected a document inside the budget to be read, got {program:?}"
        );
    }

    /// RULE: an ARCHIVE carries documents rather than commands — a MIME multipart message or
    /// the `#cloud-config-archive` list of the same — and each part is run at the cadence of
    /// the form its own part declares, beside the other parts' programs.
    #[test]
    fn an_archive_runs_each_part_at_its_own_form_s_cadence() {
        // The same three documents, written as a MIME archive and as a cloud-config archive:
        // a boothook (every boot), a script (once per instance) and a cloud-config carrying
        // one module of each cadence.
        let mime = mime_archive(&[
            ("text/cloud-boothook", "#!/bin/sh\necho from-the-boothook"),
            ("text/x-shellscript", "#!/bin/sh\necho from-the-script"),
            (
                "text/cloud-config",
                "#cloud-config\nbootcmd:\n  - echo from-the-config-every\nruncmd:\n  - echo from-the-config-once",
            ),
        ]);
        let config_archive = concat!(
            "#cloud-config-archive\n",
            "- type: text/cloud-boothook\n",
            "  content: |\n",
            "    #!/bin/sh\n",
            "    echo from-the-boothook\n",
            "- type: text/x-shellscript\n",
            "  content: |\n",
            "    #!/bin/sh\n",
            "    echo from-the-script\n",
            // An entry that declares no type is the document its own header declares.
            "- content: |\n",
            "    #cloud-config\n",
            "    bootcmd:\n",
            "      - echo from-the-config-every\n",
            "    runcmd:\n",
            "      - echo from-the-config-once\n",
        )
        .to_string();
        // (case, the archive, its every-boot commands, its first-boot commands)
        let rows = [
            (
                "a MIME multipart archive",
                mime,
                vec!["echo from-the-boothook", "echo from-the-config-every"],
                vec!["echo from-the-script", "echo from-the-config-once"],
            ),
            (
                "a cloud-config archive",
                config_archive,
                vec!["echo from-the-boothook", "echo from-the-config-every"],
                vec!["echo from-the-script", "echo from-the-config-once"],
            ),
            (
                // A part whose own type is the per-boot script form runs on every boot, and
                // one whose type names a form that would have to be FETCHED runs nothing.
                "parts declaring the other published script cadences",
                mime_archive(&[
                    ("text/x-shellscript-per-boot", "#!/bin/sh\necho per-boot"),
                    (
                        "text/x-shellscript-per-instance",
                        "#!/bin/sh\necho per-instance",
                    ),
                    ("text/x-include-url", "https://example.invalid/program"),
                ]),
                vec!["echo per-boot"],
                vec!["echo per-instance"],
            ),
            (
                // A part that declares no type at all is read as whatever its own text says
                // it is, which is how cloud-init reads a bare document.
                "a part that declares no type",
                mime_archive(&[("", "#cloud-boothook\necho untyped")]),
                vec!["echo untyped"],
                vec![],
            ),
        ];
        for (case, archive, every_boot, first_boot) in rows {
            let program = guest_program(Some(&encoded(&archive)));
            assert_eq!(
                program.every_boot, every_boot,
                "case {case:?}: expected the every-boot commands {every_boot:?}, got {:?}",
                program.every_boot
            );
            assert_eq!(
                program.first_boot, first_boot,
                "case {case:?}: expected the first-boot commands {first_boot:?}, got {:?}",
                program.first_boot
            );
        }
    }

    /// RULE: user data that arrives COMPRESSED is the document inside it, read as whatever
    /// form that document declares itself to be.
    #[test]
    fn a_compressed_user_data_is_the_document_inside_it() {
        // (case, the document that was compressed, its every-boot commands, its first-boot
        // commands)
        let rows = [
            (
                "a compressed cloud-config",
                "#cloud-config\nbootcmd:\n  - echo decompressed\n",
                vec!["echo decompressed"],
                vec![],
            ),
            (
                "a compressed boothook",
                "#cloud-boothook\necho decompressed-boothook\n",
                vec!["echo decompressed-boothook"],
                vec![],
            ),
            (
                "a compressed script",
                "#!/bin/sh\necho decompressed-script\n",
                vec![],
                vec!["echo decompressed-script"],
            ),
        ];
        for (case, document, every_boot, first_boot) in rows {
            let program = guest_program(Some(&encoded_bytes(&compressed(document))));
            assert_eq!(
                program.every_boot, every_boot,
                "case {case:?}: expected the every-boot commands {every_boot:?}, got {:?}",
                program.every_boot
            );
            assert_eq!(
                program.first_boot, first_boot,
                "case {case:?}: expected the first-boot commands {first_boot:?}, got {:?}",
                program.first_boot
            );
        }
        // Bytes that are neither text nor a stream that decompresses to any carry no program.
        assert!(
            guest_program(Some(&encoded_bytes(&[0x1f, 0x8b, 0x08, 0x00, 0x00])))
                .is_empty(),
            "a truncated compressed stream must give the guest no program"
        );
        assert!(
            guest_program(Some(&encoded_bytes(&[0xff, 0xfe, 0x00, 0x01]))).is_empty(),
            "user data that is not text at all must give the guest no program"
        );
    }

    /// RULE: a part is the document its OWN bytes carry — read through the transfer encoding
    /// the part declares, and decompressed where those bytes are compressed — so a client may
    /// send a part either way.
    #[test]
    fn a_part_is_read_through_its_own_transfer_encoding() {
        let archive = format!(
            "Content-Type: multipart/mixed; boundary=\"==PART==\"\nMIME-Version: 1.0\n\n\
             --==PART==\nContent-Type: application/octet-stream\n\
             Content-Transfer-Encoding: base64\n\n{}\n--==PART==--\n",
            encoded_bytes(&compressed(
                "#cloud-config\nbootcmd:\n  - echo compressed-part\n"
            ))
        );
        let program = guest_program(Some(&encoded(&archive)));
        assert_eq!(
            program.every_boot,
            ["echo compressed-part"],
            "expected the compressed part's own document to be read, got {program:?}"
        );
    }

    /// RULE: an archive is bounded like the one document it stands in for — how many parts are
    /// read out of it, how deeply it may carry another archive, and how many commands the
    /// whole comes to.
    #[test]
    fn what_is_read_out_of_one_archive_is_bounded() {
        let parts: Vec<(String, String)> = (0..super::MAX_GUEST_ARCHIVE_PARTS * 2)
            .map(|index| {
                (
                    "text/cloud-boothook".to_string(),
                    format!("echo part-{index}"),
                )
            })
            .collect();
        let borrowed: Vec<(&str, &str)> = parts
            .iter()
            .map(|(declared, body)| (declared.as_str(), body.as_str()))
            .collect();
        let program = guest_program(Some(&encoded(&mime_archive(&borrowed))));
        assert_eq!(
            program.every_boot.len(),
            super::MAX_GUEST_ARCHIVE_PARTS,
            "expected the parts to stop at the bound, got {:?}",
            program.every_boot
        );

        // One archive carrying another is read; one nested past the ceiling is not. Each level
        // is a PART declaring the multipart type and a boundary of its own, so the reader builds
        // the tree — the way a client nests archives.
        let program = guest_program(Some(&encoded(&nested_mime_archive(
            super::MAX_GUEST_ARCHIVE_NESTING - 1,
            "echo nested",
        ))));
        assert_eq!(
            program.every_boot,
            ["echo nested"],
            "expected an archive inside the nesting bound to be read, got {program:?}"
        );
        assert!(
            guest_program(Some(&encoded(&nested_mime_archive(
                super::MAX_GUEST_ARCHIVE_NESTING + 1,
                "echo nested",
            ))))
            .is_empty(),
            "expected an archive nested past the bound to run nothing"
        );
        // A part that declares no archive type of its own, carrying a whole archive DOCUMENT as
        // its body, is that document — the other way one archive reaches another.
        let carried = mime_archive_bounded_by("==INNER==", &[("text/cloud-boothook", "echo carried")]);
        let program = guest_program(Some(&encoded(&mime_archive(&[("", carried.as_str())]))));
        assert_eq!(
            program.every_boot,
            ["echo carried"],
            "expected a part carrying an archive document to be read as that document, got \
             {program:?}"
        );

        // The SAME bound on the other archive form, reached through a part that declares
        // itself to be one: an entry of a `#cloud-config-archive` whose `type` names that form
        // again, so the recursion runs between the two readers rather than through a document.
        let config_archive_level = |carried: &str| {
            let indented: String = carried
                .lines()
                .map(|line| format!("      {line}\n"))
                .collect();
            format!(
                "#cloud-config-archive\n- type: text/cloud-config-archive\n  content: |\n{indented}"
            )
        };
        let innermost = concat!(
            "#cloud-config-archive\n",
            "- type: text/cloud-boothook\n",
            "  content: |\n",
            "    echo nested-config-archive\n",
        )
        .to_string();
        let mut nested = innermost.clone();
        for _ in 0..2 {
            nested = config_archive_level(&nested);
        }
        let program = guest_program(Some(&encoded(&nested)));
        assert_eq!(
            program.every_boot,
            ["echo nested-config-archive"],
            "expected a cloud-config archive inside the nesting bound to be read, got \
             {program:?}"
        );
        let mut too_deep = innermost;
        for _ in 0..super::MAX_GUEST_ARCHIVE_NESTING + 2 {
            too_deep = config_archive_level(&too_deep);
        }
        assert!(
            guest_program(Some(&encoded(&too_deep))).is_empty(),
            "expected a cloud-config archive nested past the bound to run nothing"
        );

        // What a MIME message DECLARES is counted before it is parsed, so a message carrying
        // more archive declarations than every other bound admits is not read at all — not
        // even its first part, which would otherwise give the guest a command.
        let mut declarations: Vec<(&str, &str)> = vec![("text/cloud-boothook", "echo declared")];
        declarations.extend(
            (0..super::MAX_GUEST_ARCHIVE_DECLARATIONS)
                .map(|_| ("multipart/mixed", "Content-Type: multipart/mixed\n")),
        );
        assert!(
            guest_program(Some(&encoded(&mime_archive(&declarations)))).is_empty(),
            "expected a message declaring more archives than the bound admits to run nothing"
        );
        // The same first part, in a message inside that bound, IS read.
        let program = guest_program(Some(&encoded(&mime_archive(&declarations[..1]))));
        assert_eq!(
            program.every_boot,
            ["echo declared"],
            "expected a message inside the declaration bound to be read, got {program:?}"
        );
    }
}
