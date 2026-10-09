//! A ZIP writer, because the delivered kit is an archive and nothing here made
//! one.
//!
//! `ROADMAP.md` 5.8 asks for a ZIP of the whole project. The `zip` crate would
//! have done it, but it was not in the tree and adding a container crate for one
//! file format is a poor trade next to two crates that are already here:
//! `flate2` and `crc32fast`, both pulled in by `png`. The container itself is
//! about a hundred lines of a fully specified format.
//!
//! ## Determinism
//!
//! `ROADMAP.md` 5.7 requires two exports to hash identically, so nothing in the
//! archive may come from the clock. Entries carry the timestamp the caller
//! supplies — normally the project's own — and [`DOS_EPOCH`] is the default for a
//! project that has none. A ZIP that stamps "now" cannot be hashed, and an
//! unhashable artefact cannot prove it was not altered between two exports.
//!
//! Entries are written in the order given. Callers sort, so that the order is
//! visible at the call site rather than hidden here.

use std::io::Write as _;

/// 1980-01-01, the earliest a ZIP entry can express.
///
/// Used when the project carries no timestamp. Not a placeholder that leaks a
/// build date into a reproducible artifact: a fixed constant is the only value
/// that keeps two runs byte-identical when the data is.
pub const DOS_EPOCH: (u16, u16) = (33, 0);

/// One file to put in the archive.
pub struct Entry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

/// Build a ZIP archive.
///
/// `modified` is the DOS date and time pair to stamp on every entry. Pass
/// [`DOS_EPOCH`] when the project has no timestamp of its own.
#[must_use]
pub fn zip(entries: &[Entry<'_>], modified: (u16, u16)) -> Vec<u8> {
    let (date, time) = modified;
    let mut out: Vec<u8> =
        Vec::with_capacity(entries.iter().map(|e| e.data.len() + 128).sum::<usize>() + 128);
    let mut directory: Vec<u8> = Vec::new();
    let mut count = 0u16;

    for entry in entries {
        let offset = u32::try_from(out.len()).expect("архив меньше 4 ГБ");
        let crc = crc32fast::hash(entry.data);
        let name = entry.name.as_bytes();

        // Deflate is worth it for cut files and wasted for small text, and the
        // threshold is the one the format itself uses to mean "don't bother".
        let (method, payload): (u16, Vec<u8>) = if entry.data.len() >= 64 {
            let mut e =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            let compressed = e.write_all(entry.data).and_then(|()| e.finish());
            match compressed {
                Ok(p) if p.len() < entry.data.len() => (8, p),
                _ => (0, entry.data.to_vec()),
            }
        } else {
            (0, entry.data.to_vec())
        };

        let compressed = u32::try_from(payload.len()).expect("файл меньше 4 ГБ");
        let uncompressed = u32::try_from(entry.data.len()).expect("файл меньше 4 ГБ");
        let name_len = u16::try_from(name.len()).expect("имя файла разумной длины");

        // Local header, then the payload. No data descriptor: sizes are known
        // before writing because the whole entry is in memory.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&time.to_le_bytes());
        out.extend_from_slice(&date.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&compressed.to_le_bytes());
        out.extend_from_slice(&uncompressed.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra length
        out.extend_from_slice(name);
        out.extend_from_slice(&payload);

        directory.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        directory.extend_from_slice(&0x031Eu16.to_le_bytes()); // made by: unix, 3.0
        directory.extend_from_slice(&20u16.to_le_bytes()); // version needed
        directory.extend_from_slice(&0u16.to_le_bytes()); // flags
        directory.extend_from_slice(&method.to_le_bytes());
        directory.extend_from_slice(&time.to_le_bytes());
        directory.extend_from_slice(&date.to_le_bytes());
        directory.extend_from_slice(&crc.to_le_bytes());
        directory.extend_from_slice(&compressed.to_le_bytes());
        directory.extend_from_slice(&uncompressed.to_le_bytes());
        directory.extend_from_slice(&name_len.to_le_bytes());
        directory.extend_from_slice(&0u16.to_le_bytes()); // extra
        directory.extend_from_slice(&0u16.to_le_bytes()); // comment
        directory.extend_from_slice(&0u16.to_le_bytes()); // disk
        directory.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
                                                          // 0644, the mode a file extracted from a kit should land with.
        directory.extend_from_slice(&0o100_644u32.to_le_bytes());
        directory.extend_from_slice(&offset.to_le_bytes());
        directory.extend_from_slice(name);

        count = count.saturating_add(1);
    }

    let directory_offset = u32::try_from(out.len()).expect("архив меньше 4 ГБ");
    let directory_size = u32::try_from(directory.len()).expect("оглавление меньше 4 ГБ");
    out.extend_from_slice(&directory);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // this disk
    out.extend_from_slice(&0u16.to_le_bytes()); // disk with directory
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&directory_size.to_le_bytes());
    out.extend_from_slice(&directory_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length

    out
}

/// DOS date and time from a year, month, day and hour.
///
/// Returns [`DOS_EPOCH`] for anything out of range rather than a wrapped value:
/// a date outside the format's reach has no correct encoding, and guessing one
/// would put a wrong date in a reproducible artifact.
#[must_use]
pub fn dos_date(year: u16, month: u8, day: u8, hour: u8) -> (u16, u16) {
    if !(1980..=2107).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return DOS_EPOCH;
    }
    let date = ((year - 1980) << 9) | (u16::from(month) << 5) | u16::from(day);
    let time = u16::from(hour) << 11;
    (date, time)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte-wise search. `str::contains` would go through lossy decoding, and a
    /// byte sequence that happens to straddle a UTF-8 boundary there is a
    /// different question from whether the name is in the file.
    fn contains(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn the_signature_is_the_one_zip_tools_look_for() {
        let z = zip(
            &[Entry {
                name: "a.txt",
                data: b"x",
            }],
            DOS_EPOCH,
        );
        assert_eq!(&z[..4], &0x0403_4b50u32.to_le_bytes());
        assert!(z.ends_with(&[0, 0]));
    }

    #[test]
    fn every_entry_is_accounted_for_in_the_directory() {
        let entries: Vec<Entry<'_>> = ["a.svg", "b.dxf", "c.json"]
            .iter()
            .zip(["one", "two", "three"])
            .map(|(n, d)| Entry {
                name: n,
                data: d.as_bytes(),
            })
            .collect();
        let z = zip(&entries, DOS_EPOCH);
        for name in ["a.svg", "b.dxf", "c.json"] {
            assert!(
                contains(&z, name.as_bytes()).is_some(),
                "{name} отсутствует"
            );
        }
        // End of central directory, 22 bytes: signature, two disk numbers, then
        // the entry count twice (entries on this disk, then in total).
        let eocd = z.len() - 22;
        assert_eq!(&z[eocd..eocd + 4], &0x0605_4b50u32.to_le_bytes());
        let field = |at: usize| u16::from_le_bytes([z[eocd + at], z[eocd + at + 1]]);
        assert_eq!(field(8), 3, "записей на этом диске");
        assert_eq!(field(10), 3, "всего записей");
        assert_ne!(
            field(12),
            3,
            "по смещению 12 лежит размер оглавления, не счётчик"
        );
    }

    #[test]
    fn the_same_input_gives_byte_identical_output() {
        let build = || {
            zip(
                &[
                    Entry {
                        name: "project.svg",
                        data: &vec![b'x'; 5000],
                    },
                    Entry {
                        name: "manifest.json",
                        data: b"{}",
                    },
                ],
                DOS_EPOCH,
            )
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn a_large_entry_is_deflated_and_a_tiny_one_is_stored() {
        let big = vec![b'a'; 10_000];
        let z = zip(
            &[
                Entry {
                    name: "big.svg",
                    data: &big,
                },
                Entry {
                    name: "small.json",
                    data: b"{}",
                },
            ],
            DOS_EPOCH,
        );
        assert!(
            z.len() < big.len() / 4,
            "сжимаемые данные должны сжаться, архив {} байт",
            z.len()
        );
        // The stored entry keeps method 0 in its local header.
        // Local header: signature 0, version 4, flags 6, method 8, then the name
        // at offset 30. Method is therefore 22 bytes before the name.
        let stored_at = contains(&z, b"small.json").expect("имя записи");
        assert_eq!(
            u16::from_le_bytes([z[stored_at - 22], z[stored_at - 21]]),
            0,
            "короткая запись должна остаться без сжатия"
        );
    }

    #[test]
    fn a_date_outside_the_format_falls_back_rather_than_wrapping() {
        assert_eq!(dos_date(1979, 1, 1, 0), DOS_EPOCH);
        assert_eq!(dos_date(2026, 13, 1, 0), DOS_EPOCH);
        assert_eq!(
            dos_date(2026, 10, 1, 14),
            ((((2026u16 - 1980) << 9) | (10 << 5) | 1), 14 << 11)
        );
    }
}
