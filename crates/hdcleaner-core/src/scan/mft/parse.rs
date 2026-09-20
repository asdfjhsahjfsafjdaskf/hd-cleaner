//! Parsing of NTFS FILE records (Master File Table entries).
//!
//! Only the on-disk structures needed for space analysis are decoded:
//! record header + update sequence fixups, `$STANDARD_INFORMATION` (0x10),
//! `$FILE_NAME` (0x30) and `$DATA` (0x80) attribute headers, plus mapping
//! pairs (runlists) for locating the MFT itself. All reads are bounds checked:
//! a corrupt record yields `None`, never a panic.

pub const ATTR_STANDARD_INFORMATION: u32 = 0x10;
pub const ATTR_FILE_NAME: u32 = 0x30;
pub const ATTR_DATA: u32 = 0x80;
pub const ATTR_REPARSE_POINT: u32 = 0xC0;
pub const ATTR_END: u32 = 0xFFFF_FFFF;

pub const RECORD_IN_USE: u16 = 0x0001;
pub const RECORD_IS_DIR: u16 = 0x0002;

pub const ATTR_FLAG_COMPRESSED: u16 = 0x0001;
pub const ATTR_FLAG_SPARSE: u16 = 0x8000;

pub const NS_POSIX: u8 = 0;
pub const NS_WIN32: u8 = 1;
pub const NS_DOS: u8 = 2;
pub const NS_WIN32_DOS: u8 = 3;

/// NTFS protects every 512-byte stride of a record with the update sequence.
const USA_STRIDE: usize = 512;

#[inline]
fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}
#[inline]
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}
#[inline]
fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
}

/// Validates the "FILE" signature and applies the update sequence array.
/// Returns false for unused/corrupt records.
pub fn apply_fixup(rec: &mut [u8]) -> bool {
    if rec.len() < 48 || &rec[0..4] != b"FILE" {
        return false;
    }
    let (Some(usa_off), Some(usa_count)) = (u16_at(rec, 4), u16_at(rec, 6)) else { return false };
    let (usa_off, usa_count) = (usa_off as usize, usa_count as usize);
    if usa_count < 2 || usa_off + usa_count * 2 > rec.len() || (usa_count - 1) * USA_STRIDE > rec.len() {
        return false;
    }
    let usn = [rec[usa_off], rec[usa_off + 1]];
    for i in 1..usa_count {
        let end = i * USA_STRIDE - 2;
        if rec[end..end + 2] != usn {
            return false; // torn write / corruption
        }
        rec[end] = rec[usa_off + i * 2];
        rec[end + 1] = rec[usa_off + i * 2 + 1];
    }
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileName {
    /// Record number of the parent directory (low 48 bits of the reference).
    pub parent: u64,
    pub namespace: u8,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedRecord {
    pub in_use: bool,
    pub is_dir: bool,
    /// Base record number when this is an extension record.
    pub base: Option<u64>,
    pub hard_link_count: u16,
    pub created: u64,
    pub modified: u64,
    pub accessed: u64,
    pub attributes: u32,
    pub has_std_info: bool,
    pub names: Vec<FileName>,
    /// Logical size of the unnamed data stream, if this record holds its header.
    pub data_size: Option<u64>,
    /// Clusters actually allocated (non-sparse runs) by the data stream
    /// fragments stored in this record.
    pub alloc_clusters: u64,
    /// Header-declared bytes for fragments whose runlist could not be decoded.
    pub alloc_extra_bytes: u64,
    pub sparse: bool,
    pub compressed: bool,
    pub reparse: bool,
}

/// Parse an already fixed-up FILE record.
pub fn parse_record(rec: &[u8]) -> Option<ParsedRecord> {
    let flags = u16_at(rec, 0x16)?;
    let first_attr = u16_at(rec, 0x14)? as usize;
    let bytes_in_use = (u32_at(rec, 0x18)? as usize).min(rec.len());
    let base_ref = u64_at(rec, 0x20)? & 0x0000_FFFF_FFFF_FFFF;
    let mut out = ParsedRecord {
        in_use: flags & RECORD_IN_USE != 0,
        is_dir: flags & RECORD_IS_DIR != 0,
        base: (base_ref != 0).then_some(base_ref),
        hard_link_count: u16_at(rec, 0x12).unwrap_or(0),
        ..Default::default()
    };
    if !out.in_use {
        return Some(out);
    }

    let mut off = first_attr;
    // A record can hold at most a few dozen attributes; bound the loop anyway.
    for _ in 0..256 {
        if off + 16 > bytes_in_use {
            break;
        }
        let ty = u32_at(rec, off)?;
        if ty == ATTR_END {
            break;
        }
        let len = u32_at(rec, off + 4)? as usize;
        if len < 16 || off + len > bytes_in_use {
            break;
        }
        let a = &rec[off..off + len];
        let non_resident = a[8] != 0;
        let name_len = a[9];
        let aflags = u16_at(a, 0x0C)?;

        match ty {
            ATTR_STANDARD_INFORMATION if !non_resident => {
                if let Some(v) = resident_value(a) {
                    if v.len() >= 0x24 {
                        out.created = u64_at(v, 0x00)?;
                        out.modified = u64_at(v, 0x08)?;
                        out.accessed = u64_at(v, 0x18)?;
                        out.attributes = u32_at(v, 0x20)?;
                        out.has_std_info = true;
                    }
                }
            }
            ATTR_FILE_NAME if !non_resident => {
                if let Some(v) = resident_value(a) {
                    if v.len() >= 0x42 {
                        let parent = u64_at(v, 0)? & 0x0000_FFFF_FFFF_FFFF;
                        let n = v[0x40] as usize;
                        let ns = v[0x41];
                        if let Some(raw) = v.get(0x42..0x42 + n * 2) {
                            let units: Vec<u16> =
                                raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                            out.names.push(FileName {
                                parent,
                                namespace: ns,
                                name: String::from_utf16_lossy(&units),
                            });
                        }
                    }
                }
            }
            ATTR_DATA => {
                let unnamed = name_len == 0;
                if aflags & ATTR_FLAG_SPARSE != 0 {
                    out.sparse = true;
                }
                if aflags & ATTR_FLAG_COMPRESSED != 0 {
                    out.compressed = true;
                }
                if non_resident {
                    let lowest_vcn = u64_at(a, 0x10)?;
                    // Space actually used on disk = clusters of the non-sparse
                    // runs. Counted per fragment, so streams split across
                    // extension records add up correctly, and sparse streams
                    // without the sparse flag (e.g. $BadClus:$Bad, as large as
                    // the volume) count only real clusters.
                    let map_off = u16_at(a, 0x20)? as usize;
                    match a.get(map_off..).and_then(parse_runlist) {
                        Some(runs) => {
                            out.alloc_clusters += runs.iter().filter(|r| r.lcn.is_some()).map(|r| r.clusters).sum::<u64>();
                        }
                        None if lowest_vcn == 0 && a.len() >= 0x30 => {
                            // Unreadable runlist: fall back to the header value.
                            out.alloc_extra_bytes += u64_at(a, 0x28)?;
                        }
                        None => {}
                    }
                    if lowest_vcn == 0 && a.len() >= 0x40 && unnamed {
                        out.data_size = Some(u64_at(a, 0x30)?);
                    }
                } else if unnamed {
                    // Resident data lives inside the MFT record: no clusters.
                    out.data_size = Some(u32_at(a, 0x10)? as u64);
                }
            }
            ATTR_REPARSE_POINT => out.reparse = true,
            _ => {}
        }
        off += len;
    }
    Some(out)
}

fn resident_value(a: &[u8]) -> Option<&[u8]> {
    let len = u32_at(a, 0x10)? as usize;
    let off = u16_at(a, 0x14)? as usize;
    a.get(off..off + len)
}

/// One extent of a non-resident attribute: `lcn == None` means sparse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub lcn: Option<u64>,
    pub clusters: u64,
}

/// Decode NTFS mapping pairs.
pub fn parse_runlist(mut b: &[u8]) -> Option<Vec<Run>> {
    let mut runs = Vec::new();
    let mut lcn: i64 = 0;
    while let Some(&header) = b.first() {
        if header == 0 {
            break;
        }
        let len_size = (header & 0x0F) as usize;
        let off_size = (header >> 4) as usize;
        if len_size == 0 || len_size > 8 || off_size > 8 || b.len() < 1 + len_size + off_size {
            return None;
        }
        let mut length: u64 = 0;
        for i in 0..len_size {
            length |= (b[1 + i] as u64) << (8 * i);
        }
        let lcn_value = if off_size == 0 {
            None
        } else {
            let mut delta: i64 = 0;
            for i in 0..off_size {
                delta |= (b[1 + len_size + i] as i64) << (8 * i);
            }
            // sign-extend
            let shift = 64 - 8 * off_size as u32;
            delta = (delta << shift) >> shift;
            lcn = lcn.checked_add(delta)?;
            if lcn < 0 {
                return None;
            }
            Some(lcn as u64)
        };
        runs.push(Run { lcn: lcn_value, clusters: length });
        b = &b[1 + len_size + off_size..];
    }
    Some(runs)
}

/// Locate the runlist of the unnamed `$DATA` attribute (lowest VCN 0) in a
/// fixed-up record. Used on record 0 to find the extents of the MFT itself.
/// Returns (runs, data_size). `None` if the attribute is absent or the record
/// uses an attribute list (the caller then reports the MFT as unsupported).
pub fn unnamed_data_runs(rec: &[u8]) -> Option<(Vec<Run>, u64)> {
    let first_attr = u16_at(rec, 0x14)? as usize;
    let bytes_in_use = (u32_at(rec, 0x18)? as usize).min(rec.len());
    let mut off = first_attr;
    for _ in 0..256 {
        if off + 16 > bytes_in_use {
            return None;
        }
        let ty = u32_at(rec, off)?;
        if ty == ATTR_END {
            return None;
        }
        let len = u32_at(rec, off + 4)? as usize;
        if len < 16 || off + len > bytes_in_use {
            return None;
        }
        let a = &rec[off..off + len];
        if ty == ATTR_DATA && a[8] != 0 && a[9] == 0 && u64_at(a, 0x10)? == 0 {
            let map_off = u16_at(a, 0x20)? as usize;
            let size = u64_at(a, 0x30)?;
            return Some((parse_runlist(a.get(map_off..)?)?, size));
        }
        off += len;
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a synthetic 1024-byte FILE record for tests.
    pub struct RecordBuilder {
        buf: Vec<u8>,
        off: usize,
    }

    impl RecordBuilder {
        pub fn new(flags: u16, base: u64) -> Self {
            let mut buf = vec![0u8; 1024];
            buf[0..4].copy_from_slice(b"FILE");
            buf[4..6].copy_from_slice(&0x30u16.to_le_bytes()); // USA offset
            buf[6..8].copy_from_slice(&3u16.to_le_bytes()); // USA count (1 + 2 strides)
            buf[0x14..0x16].copy_from_slice(&0x38u16.to_le_bytes()); // first attr
            buf[0x16..0x18].copy_from_slice(&flags.to_le_bytes());
            buf[0x1C..0x20].copy_from_slice(&1024u32.to_le_bytes());
            buf[0x20..0x28].copy_from_slice(&base.to_le_bytes());
            RecordBuilder { buf, off: 0x38 }
        }

        fn push_attr(&mut self, bytes: Vec<u8>) {
            let len = bytes.len().div_ceil(8) * 8;
            self.buf[self.off..self.off + bytes.len()].copy_from_slice(&bytes);
            self.buf[self.off + 4..self.off + 8].copy_from_slice(&(len as u32).to_le_bytes());
            self.off += len;
        }

        fn resident(ty: u32, name: &[u16], value: &[u8], flags: u16) -> Vec<u8> {
            let name_off = 0x18usize;
            let val_off = (name_off + name.len() * 2).div_ceil(8) * 8;
            let mut a = vec![0u8; val_off + value.len()];
            a[0..4].copy_from_slice(&ty.to_le_bytes());
            a[9] = name.len() as u8;
            a[0x0A..0x0C].copy_from_slice(&(name_off as u16).to_le_bytes());
            a[0x0C..0x0E].copy_from_slice(&flags.to_le_bytes());
            a[0x10..0x14].copy_from_slice(&(value.len() as u32).to_le_bytes());
            a[0x14..0x16].copy_from_slice(&(val_off as u16).to_le_bytes());
            for (i, c) in name.iter().enumerate() {
                a[name_off + i * 2..name_off + i * 2 + 2].copy_from_slice(&c.to_le_bytes());
            }
            a[val_off..].copy_from_slice(value);
            a
        }

        pub fn std_info(mut self, created: u64, modified: u64, attrs: u32) -> Self {
            let mut v = vec![0u8; 0x48];
            v[0..8].copy_from_slice(&created.to_le_bytes());
            v[8..16].copy_from_slice(&modified.to_le_bytes());
            v[0x18..0x20].copy_from_slice(&modified.to_le_bytes());
            v[0x20..0x24].copy_from_slice(&attrs.to_le_bytes());
            self.push_attr(Self::resident(ATTR_STANDARD_INFORMATION, &[], &v, 0));
            self
        }

        pub fn file_name(mut self, parent: u64, ns: u8, name: &str) -> Self {
            let units: Vec<u16> = name.encode_utf16().collect();
            let mut v = vec![0u8; 0x42 + units.len() * 2];
            v[0..8].copy_from_slice(&(parent | (1u64 << 48)).to_le_bytes());
            v[0x40] = units.len() as u8;
            v[0x41] = ns;
            for (i, c) in units.iter().enumerate() {
                v[0x42 + i * 2..0x44 + i * 2].copy_from_slice(&c.to_le_bytes());
            }
            self.push_attr(Self::resident(ATTR_FILE_NAME, &[], &v, 0));
            self
        }

        pub fn resident_data(mut self, name: &str, len: usize) -> Self {
            let n: Vec<u16> = name.encode_utf16().collect();
            self.push_attr(Self::resident(ATTR_DATA, &n, &vec![0xAB; len], 0));
            self
        }

        pub fn nonresident_data(
            mut self,
            name: &str,
            lowest_vcn: u64,
            allocated: u64,
            real: u64,
            compressed: Option<u64>,
            runlist: &[u8],
        ) -> Self {
            let n: Vec<u16> = name.encode_utf16().collect();
            let header = if compressed.is_some() { 0x48 } else { 0x40 };
            let name_off = header;
            let map_off = (name_off + n.len() * 2).div_ceil(8) * 8;
            let mut a = vec![0u8; map_off + runlist.len() + 1];
            a[0..4].copy_from_slice(&ATTR_DATA.to_le_bytes());
            a[8] = 1;
            a[9] = n.len() as u8;
            a[0x0A..0x0C].copy_from_slice(&(name_off as u16).to_le_bytes());
            let flags = if compressed.is_some() { ATTR_FLAG_SPARSE } else { 0 };
            a[0x0C..0x0E].copy_from_slice(&flags.to_le_bytes());
            a[0x10..0x18].copy_from_slice(&lowest_vcn.to_le_bytes());
            a[0x20..0x22].copy_from_slice(&(map_off as u16).to_le_bytes());
            a[0x28..0x30].copy_from_slice(&allocated.to_le_bytes());
            a[0x30..0x38].copy_from_slice(&real.to_le_bytes());
            a[0x38..0x40].copy_from_slice(&real.to_le_bytes());
            if let Some(c) = compressed {
                a[0x40..0x48].copy_from_slice(&c.to_le_bytes());
            }
            for (i, c) in n.iter().enumerate() {
                a[name_off + i * 2..name_off + i * 2 + 2].copy_from_slice(&c.to_le_bytes());
            }
            a[map_off..map_off + runlist.len()].copy_from_slice(runlist);
            self.push_attr(a);
            self
        }

        /// Finish: end marker, bytes-in-use and update sequence protection.
        pub fn build(mut self) -> Vec<u8> {
            self.buf[self.off..self.off + 4].copy_from_slice(&ATTR_END.to_le_bytes());
            let used = (self.off + 8) as u32;
            self.buf[0x18..0x1C].copy_from_slice(&used.to_le_bytes());
            // USN = 0x0007; save original stride tails into the USA.
            let usn = 0x0007u16.to_le_bytes();
            self.buf[0x30..0x32].copy_from_slice(&usn);
            for i in 1..3 {
                let end = i * 512 - 2;
                let (a, b) = (self.buf[end], self.buf[end + 1]);
                self.buf[0x30 + i * 2] = a;
                self.buf[0x30 + i * 2 + 1] = b;
                self.buf[end..end + 2].copy_from_slice(&usn);
            }
            self.buf
        }
    }

    #[test]
    fn fixup_detects_torn_records() {
        let mut rec = RecordBuilder::new(RECORD_IN_USE, 0).std_info(1, 2, 0x20).build();
        let mut torn = rec.clone();
        assert!(apply_fixup(&mut rec));
        torn[1022] ^= 0xFF;
        assert!(!apply_fixup(&mut torn));
        let mut bad = vec![0u8; 1024];
        assert!(!apply_fixup(&mut bad));
    }

    #[test]
    fn parses_file_record() {
        let mut rec = RecordBuilder::new(RECORD_IN_USE, 0)
            .std_info(111, 222, 0x20)
            .file_name(5, NS_DOS, "LONGFI~1.MP4")
            .file_name(5, NS_WIN32, "Long file name ção.mp4")
            .nonresident_data("", 0, 8192, 5000, None, &[0x11, 0x02, 0x10])
            .resident_data("Zone.Identifier", 26)
            .build();
        assert!(apply_fixup(&mut rec));
        let p = parse_record(&rec).unwrap();
        assert!(p.in_use && !p.is_dir);
        assert_eq!(p.created, 111);
        assert_eq!(p.modified, 222);
        assert_eq!(p.attributes, 0x20);
        assert_eq!(p.names.len(), 2);
        assert_eq!(p.names[1].name, "Long file name ção.mp4");
        assert_eq!(p.names[1].parent, 5, "sequence number must be masked off");
        assert_eq!(p.data_size, Some(5000));
        assert_eq!(p.alloc_clusters, 2);
        assert_eq!(p.base, None);
    }

    #[test]
    fn sparse_stream_uses_compressed_size() {
        let mut rec = RecordBuilder::new(RECORD_IN_USE, 0)
            .std_info(1, 1, 0x220)
            .file_name(5, NS_WIN32, "sparse.bin")
            .nonresident_data("", 0, 1 << 30, 1 << 30, Some(65536), &[0x11, 0x01, 0x10])
            .build();
        assert!(apply_fixup(&mut rec));
        let p = parse_record(&rec).unwrap();
        assert_eq!(p.data_size, Some(1 << 30));
        assert_eq!(p.alloc_clusters, 1, "only the real (non-sparse) run counts");
        assert!(p.sparse);
    }

    #[test]
    fn extension_record_and_resident_data() {
        let mut rec = RecordBuilder::new(RECORD_IN_USE, 42 | (3 << 48))
            .nonresident_data("", 0, 4096, 4000, None, &[0x11, 0x01, 0x10])
            .build();
        assert!(apply_fixup(&mut rec));
        let p = parse_record(&rec).unwrap();
        assert_eq!(p.base, Some(42));
        let mut small = RecordBuilder::new(RECORD_IN_USE, 0).resident_data("", 100).build();
        assert!(apply_fixup(&mut small));
        let p = parse_record(&small).unwrap();
        assert_eq!(p.data_size, Some(100));
        assert_eq!(p.alloc_clusters, 0, "resident data uses no clusters");
    }

    #[test]
    fn continuation_fragment_adds_clusters_but_no_size() {
        // Later fragment of a stream split across records: its clusters are
        // real disk usage, but the logical size lives in the first fragment.
        let mut rec = RecordBuilder::new(RECORD_IN_USE, 42)
            .nonresident_data("", 100, 0, 0, None, &[0x11, 0x03, 0x10])
            .build();
        assert!(apply_fixup(&mut rec));
        let p = parse_record(&rec).unwrap();
        assert_eq!(p.data_size, None);
        assert_eq!(p.alloc_clusters, 3);
    }

    #[test]
    fn unflagged_sparse_stream_counts_only_real_clusters() {
        // Like $BadClus:$Bad: header says allocated = whole volume, no sparse
        // flag, runlist is one sparse run plus a single real cluster.
        let mut rec = RecordBuilder::new(RECORD_IN_USE, 0)
            .std_info(1, 1, 0x6)
            .file_name(5, NS_WIN32_DOS, "$BadClus")
            .resident_data("", 0)
            .nonresident_data("$Bad", 0, 500 << 30, 500 << 30, None, &[0x04, 0x00, 0x00, 0x00, 0x08, 0x11, 0x01, 0x20])
            .build();
        assert!(apply_fixup(&mut rec));
        let p = parse_record(&rec).unwrap();
        assert_eq!(p.alloc_clusters, 1);
        assert_eq!(p.data_size, Some(0));
    }

    #[test]
    fn runlists() {
        // 0x21: 1-byte length, 2-byte offset. run1: 0x18 clusters at 0x5634;
        // run2: 0x10 clusters at delta -0x100 (0xFF00); run3 sparse 0x08.
        let rl = [0x21, 0x18, 0x34, 0x56, 0x21, 0x10, 0x00, 0xFF, 0x01, 0x08, 0x00];
        let runs = parse_runlist(&rl).unwrap();
        assert_eq!(
            runs,
            vec![
                Run { lcn: Some(0x5634), clusters: 0x18 },
                Run { lcn: Some(0x5534), clusters: 0x10 },
                Run { lcn: None, clusters: 0x08 },
            ]
        );
        assert!(parse_runlist(&[0x21, 0x18]).is_none());
    }

    #[test]
    fn finds_mft_data_runs() {
        let mut rec = RecordBuilder::new(RECORD_IN_USE, 0)
            .std_info(1, 1, 6)
            .file_name(5, NS_WIN32_DOS, "$MFT")
            .nonresident_data("", 0, 0x40000, 0x40000, None, &[0x21, 0x40, 0x00, 0x0C])
            .build();
        assert!(apply_fixup(&mut rec));
        let (runs, size) = unnamed_data_runs(&rec).unwrap();
        assert_eq!(size, 0x40000);
        assert_eq!(runs, vec![Run { lcn: Some(0x0C00), clusters: 0x40 }]);
    }

    #[test]
    fn garbage_never_panics() {
        let mut seed = 0x1234_5678u32;
        for _ in 0..2000 {
            let mut buf = vec![0u8; 1024];
            for b in buf.iter_mut() {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *b = seed as u8;
            }
            buf[0..4].copy_from_slice(b"FILE");
            let _ = apply_fixup(&mut buf);
            let _ = parse_record(&buf);
            let _ = unnamed_data_runs(&buf);
        }
    }
}
