//! Installed font faces kept between CLI calls, so a render after boot does not
//! open every system font file. The cached list is used only while a listing of
//! the font folders (every entry's path, size and modification time) matches
//! the one taken when it was written; any change rescans. Windows and macOS
//! only: on Linux the folders come from fontconfig's configuration, so it scans.
use resvg::usvg::fontdb::{Database, FaceInfo, Language, Source, Stretch, Style, Weight};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"PNFC";
const FORMAT: u32 = 1;
/// Font folders with more entries than this are scanned, not fingerprinted.
const MAX_ENTRIES: usize = 65536;

/// The installed faces in the order `Database::load_system_fonts` finds them.
pub fn system_fonts() -> Database {
    let scan = || {
        let mut db = Database::new();
        db.load_system_fonts();
        db
    };
    if !crate::render_cache::enabled() {
        return scan();
    }
    let (Some(roots), Some(file)) = (roots(), location()) else {
        return scan();
    };
    let Some(print) = fingerprint(&roots) else {
        return scan();
    };
    if let Some(db) = std::fs::read(&file)
        .ok()
        .and_then(|bytes| decode(&bytes, &print))
    {
        return db;
    }
    let db = scan();
    if let Some(bytes) = encode(&db, &print) {
        let _ = write(&file, &bytes);
    }
    db
}

/// The folders `load_system_fonts` reads (fontdb 0.22).
fn roots() -> Option<Vec<PathBuf>> {
    #[cfg(target_os = "windows")]
    {
        let system = std::env::var_os("SYSTEMROOT").map_or_else(
            || PathBuf::from("C:\\Windows\\Fonts\\"),
            |root| Path::new(&root).join("Fonts"),
        );
        let mut roots = vec![system];
        if let Ok(home) = std::env::var("USERPROFILE") {
            let home = Path::new(&home);
            roots.push(home.join("AppData\\Local\\Microsoft\\Windows\\Fonts"));
            roots.push(home.join("AppData\\Roaming\\Microsoft\\Windows\\Fonts"));
        }
        Some(roots)
    }
    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![
            PathBuf::from("/Library/Fonts"),
            PathBuf::from("/System/Library/Fonts"),
            // Downloadable fonts live in subfolders of this one.
            PathBuf::from("/System/Library/AssetsV2"),
            PathBuf::from("/Network/Library/Fonts"),
        ];
        if let Ok(home) = std::env::var("HOME") {
            roots.push(Path::new(&home).join("Library/Fonts"));
        }
        Some(roots)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

fn location() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    #[cfg(target_os = "macos")]
    let base = Path::new(&std::env::var_os("HOME")?).join("Library/Caches");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base: PathBuf = return None;
    Some(base.join("pentool").join("fonts.bin"))
}

/// A hash of every entry under `roots`, following links like fontdb does.
fn fingerprint(roots: &[PathBuf]) -> Option<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(FORMAT.to_le_bytes());
    hasher.update(crate::render_cache::namespace().as_bytes());
    let mut count = 0;
    for root in roots {
        hasher.update(b"\0root\0");
        hasher.update(root.to_str()?.as_bytes());
        walk(root, &mut hasher, &mut count, 0)?;
    }
    Some(hasher.finalize().into())
}

fn walk(dir: &Path, hasher: &mut Sha256, count: &mut usize, depth: u32) -> Option<()> {
    if depth > 16 {
        return None;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        hasher.update(b"\0missing");
        return Some(());
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        *count += 1;
        if *count > MAX_ENTRIES {
            return None;
        }
        let path = entry.path();
        hasher.update(b"\0entry\0");
        hasher.update(path.to_str()?.as_bytes());
        // Directory listings carry size and time without opening each file;
        // links are followed to their target.
        let metadata = match entry.file_type() {
            Ok(kind) if kind.is_symlink() => std::fs::metadata(&path),
            _ => entry.metadata(),
        };
        let Ok(metadata) = metadata else {
            hasher.update(b"\0unreadable");
            continue;
        };
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        hasher.update(metadata.len().to_le_bytes());
        hasher.update(modified.as_nanos().to_le_bytes());
        if metadata.is_dir() {
            hasher.update(b"\0dir");
            walk(&path, hasher, count, depth + 1)?;
        }
    }
    Some(())
}

const STRETCHES: [Stretch; 9] = [
    Stretch::UltraCondensed,
    Stretch::ExtraCondensed,
    Stretch::Condensed,
    Stretch::SemiCondensed,
    Stretch::Normal,
    Stretch::SemiExpanded,
    Stretch::Expanded,
    Stretch::ExtraExpanded,
    Stretch::UltraExpanded,
];
const STYLES: [Style; 3] = [Style::Normal, Style::Italic, Style::Oblique];

/// fontdb names its languages by Windows language ID, so the ID round-trips.
fn language(id: u16) -> Language {
    ttf_parser::name::Name {
        platform_id: ttf_parser::PlatformId::Windows,
        encoding_id: 0,
        language_id: id,
        name_id: 0,
        name: &[],
    }
    .language()
}

fn put(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
}

fn encode(db: &Database, print: &[u8; 32]) -> Option<Vec<u8>> {
    let mut languages: Vec<(Language, u16)> = Vec::new();
    for id in 0..=u16::MAX {
        let found = language(id);
        if !languages.iter().any(|(known, _)| *known == found) {
            languages.push((found, id));
        }
    }
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&FORMAT.to_le_bytes());
    out.extend_from_slice(print);
    out.extend_from_slice(&(db.len() as u32).to_le_bytes());
    for face in db.faces() {
        let Source::File(path) = &face.source else {
            return None;
        };
        put(&mut out, path.to_str()?);
        out.extend_from_slice(&face.index.to_le_bytes());
        out.extend_from_slice(&(face.families.len() as u32).to_le_bytes());
        for (name, lang) in &face.families {
            put(&mut out, name);
            let id = languages.iter().find(|(known, _)| known == lang)?.1;
            out.extend_from_slice(&id.to_le_bytes());
        }
        put(&mut out, &face.post_script_name);
        out.push(STYLES.iter().position(|s| *s == face.style)? as u8);
        out.extend_from_slice(&face.weight.0.to_le_bytes());
        out.push(STRETCHES.iter().position(|s| *s == face.stretch)? as u8);
        out.push(u8::from(face.monospaced));
    }
    let sum = crate::render_cache::checksum(&out);
    out.extend_from_slice(&sum.to_le_bytes());
    Some(out)
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn text(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        String::from_utf8(self.take(len)?.to_vec()).ok()
    }
}

fn decode(bytes: &[u8], print: &[u8; 32]) -> Option<Database> {
    let (body, sum) = bytes.split_at_checked(bytes.len().checked_sub(8)?)?;
    if crate::render_cache::checksum(body) != u64::from_le_bytes(sum.try_into().ok()?) {
        return None;
    }
    let mut reader = Reader(body);
    if reader.take(4)? != MAGIC || reader.u32()? != FORMAT || reader.take(32)? != print {
        return None;
    }
    let mut db = Database::new();
    for _ in 0..reader.u32()? {
        let source = Source::File(PathBuf::from(reader.text()?));
        let index = reader.u32()?;
        let families = (0..reader.u32()?)
            .map(|_| Some((reader.text()?, language(reader.u16()?))))
            .collect::<Option<Vec<_>>>()?;
        db.push_face_info(FaceInfo {
            id: resvg::usvg::fontdb::ID::dummy(),
            source,
            index,
            families,
            post_script_name: reader.text()?,
            style: *STYLES.get(usize::from(reader.u8()?))?,
            weight: Weight(reader.u16()?),
            stretch: *STRETCHES.get(usize::from(reader.u8()?))?,
            monospaced: reader.u8()? != 0,
        });
    }
    reader.0.is_empty().then_some(db)
}

fn write(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = file.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".fonts.bin.{}", std::process::id()));
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, file).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn faces(db: &Database) -> Vec<String> {
        db.faces().map(|face| format!("{face:?}")).collect()
    }

    #[test]
    fn cached_faces_match_a_scan_exactly() {
        let dir = std::env::temp_dir().join(format!("pentool-font-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (i, (_, _, _, bytes)) in crate::fonts::BUNDLED.iter().enumerate() {
            std::fs::write(dir.join(format!("face-{i}.ttf")), bytes).unwrap();
        }
        let mut scanned = Database::new();
        scanned.load_fonts_dir(&dir);
        assert!(scanned.len() >= crate::fonts::BUNDLED.len());
        let print = [7; 32];
        let bytes = encode(&scanned, &print).unwrap();
        assert_eq!(faces(&decode(&bytes, &print).unwrap()), faces(&scanned));
        // A different fingerprint, a damaged byte or a short file is a miss.
        assert!(decode(&bytes, &[8; 32]).is_none());
        let mut damaged = bytes.clone();
        damaged[40] ^= 1;
        assert!(decode(&damaged, &print).is_none());
        assert!(decode(&bytes[..bytes.len() - 1], &print).is_none());
        // Adding a font changes the fingerprint.
        let before = fingerprint(std::slice::from_ref(&dir)).unwrap();
        assert_eq!(fingerprint(std::slice::from_ref(&dir)).unwrap(), before);
        std::fs::write(dir.join("extra.ttf"), crate::fonts::BUNDLED[0].3).unwrap();
        assert_ne!(fingerprint(std::slice::from_ref(&dir)).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installed_fonts_round_trip_through_the_cache() {
        let mut scanned = Database::new();
        scanned.load_system_fonts();
        let encoded = encode(&scanned, &[1; 32]);
        if roots().is_some() {
            assert!(encoded.is_some(), "installed fonts must be cacheable");
        }
        if let Some(bytes) = encoded {
            assert_eq!(faces(&decode(&bytes, &[1; 32]).unwrap()), faces(&scanned));
        }
    }
}
