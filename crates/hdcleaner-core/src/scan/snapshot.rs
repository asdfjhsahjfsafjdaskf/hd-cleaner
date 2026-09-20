//! Binary snapshot format (`.hdcs`) for scan results.
//!
//! ```text
//! "HDCSNAP1"                      8 bytes, uncompressed
//! LZ4 frame {
//!   u32 version
//!   u32 meta_len, meta (JSON of ScanMeta)
//!   u64 node_count, node_count * NODE_BYTES little-endian records
//!   u64 names_len, names (UTF-8 pool)
//!   u64 children_len, children_len * u32
//!   u32 ext_count, ext_count * (u8 len, bytes)
//! }
//! ```
//! Snapshots are treated as untrusted input on load: every index is bounds
//! checked before a `ScanTree` is constructed.

use super::model::{Node, ScanMeta, ScanTree};
use crate::branding::SNAPSHOT_MAGIC;
use crate::{AppError, Result};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

const VERSION: u32 = 1;
const NODE_BYTES: usize = 76;
const MAX_META: usize = 64 * 1024 * 1024;

fn io_err(p: &Path) -> impl Fn(std::io::Error) -> AppError + '_ {
    move |e| AppError::io("snapshot I/O", Some(p), e)
}

pub fn save(tree: &ScanTree, path: &Path) -> Result<()> {
    let tmp = path.with_extension("hdcs.partial");
    let file = std::fs::File::create(&tmp).map_err(io_err(&tmp))?;
    let mut w = BufWriter::with_capacity(1 << 20, file);
    write_to(tree, &mut w).map_err(io_err(&tmp))?;
    let file = w.into_inner().map_err(|e| AppError::io("flushing snapshot", Some(&tmp), e.into_error()))?;
    file.sync_all().map_err(io_err(&tmp))?;
    drop(file);
    // Atomic replace so a crash never leaves a truncated snapshot behind.
    std::fs::rename(&tmp, path).map_err(io_err(path))?;
    Ok(())
}

/// Stream a snapshot (magic + compressed body) into any writer.
pub fn write_to(tree: &ScanTree, w: &mut impl Write) -> std::io::Result<()> {
    w.write_all(SNAPSHOT_MAGIC)?;
    let mut enc = lz4_flex::frame::FrameEncoder::new(w);
    write_body(tree, &mut enc)?;
    enc.finish().map_err(std::io::Error::other)?;
    Ok(())
}

fn write_body(tree: &ScanTree, w: &mut impl Write) -> std::io::Result<()> {
    w.write_all(&VERSION.to_le_bytes())?;
    let meta = serde_json::to_vec(&tree.meta).map_err(std::io::Error::other)?;
    w.write_all(&(meta.len() as u32).to_le_bytes())?;
    w.write_all(&meta)?;
    w.write_all(&(tree.nodes.len() as u64).to_le_bytes())?;
    let mut rec = [0u8; NODE_BYTES];
    for n in &tree.nodes {
        encode_node(n, &mut rec);
        w.write_all(&rec)?;
    }
    w.write_all(&(tree.names.len() as u64).to_le_bytes())?;
    w.write_all(&tree.names)?;
    w.write_all(&(tree.children.len() as u64).to_le_bytes())?;
    for c in &tree.children {
        w.write_all(&c.to_le_bytes())?;
    }
    w.write_all(&(tree.exts.len() as u32).to_le_bytes())?;
    for e in &tree.exts {
        let b = e.as_bytes();
        w.write_all(&[b.len().min(255) as u8])?;
        w.write_all(&b[..b.len().min(255)])?;
    }
    Ok(())
}

fn encode_node(n: &Node, b: &mut [u8; NODE_BYTES]) {
    let mut o = 0;
    let mut put = |bytes: &[u8]| {
        b[o..o + bytes.len()].copy_from_slice(bytes);
        o += bytes.len();
    };
    put(&n.parent.to_le_bytes());
    put(&n.name_off.to_le_bytes());
    put(&n.size.to_le_bytes());
    put(&n.alloc.to_le_bytes());
    put(&n.created.to_le_bytes());
    put(&n.modified.to_le_bytes());
    put(&n.accessed.to_le_bytes());
    put(&n.files.to_le_bytes());
    put(&n.dirs.to_le_bytes());
    put(&n.child_start.to_le_bytes());
    put(&n.child_count.to_le_bytes());
    put(&n.attributes.to_le_bytes());
    put(&n.name_len.to_le_bytes());
    put(&n.ext.to_le_bytes());
    put(&n.flags.to_le_bytes());
    put(&n.links.to_le_bytes());
}

fn decode_node(b: &[u8]) -> Node {
    let u32_ = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let u64_ = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    let u16_ = |o: usize| u16::from_le_bytes(b[o..o + 2].try_into().unwrap());
    Node {
        parent: u32_(0),
        name_off: u32_(4),
        size: u64_(8),
        alloc: u64_(16),
        created: u64_(24),
        modified: u64_(32),
        accessed: u64_(40),
        files: u32_(48),
        dirs: u32_(52),
        child_start: u32_(56),
        child_count: u32_(60),
        attributes: u32_(64),
        name_len: u16_(68),
        ext: u16_(70),
        flags: u16_(72),
        links: u16_(74),
    }
}

pub fn load(path: &Path) -> Result<ScanTree> {
    let file = std::fs::File::open(path).map_err(io_err(path))?;
    let mut r = BufReader::with_capacity(1 << 20, file);
    read_from(&mut r)
}

/// Read a snapshot from any reader (file, pipe).
pub fn read_from(r: &mut impl Read) -> Result<ScanTree> {
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic).map_err(|e| AppError::Corrupt(format!("reading snapshot header: {e}")))?;
    if &magic != SNAPSHOT_MAGIC {
        return Err(AppError::Corrupt("not a snapshot file (bad magic)".into()));
    }
    let mut dec = lz4_flex::frame::FrameDecoder::new(r);
    read_body(&mut dec).map_err(|e| match e {
        AppError::Io { source, .. } => AppError::Corrupt(format!("truncated or corrupt snapshot: {source}")),
        other => other,
    })
}

fn read_body(r: &mut impl Read) -> Result<ScanTree> {
    let rd = |r: &mut dyn Read, n: usize| -> Result<Vec<u8>> {
        let mut v = Vec::new();
        r.take(n as u64).read_to_end(&mut v).map_err(|e| AppError::io("reading snapshot", None, e))?;
        if v.len() != n {
            return Err(AppError::Corrupt("truncated snapshot".into()));
        }
        Ok(v)
    };
    let u32r = |r: &mut dyn Read| -> Result<u32> { Ok(u32::from_le_bytes(rd(r, 4)?.try_into().unwrap())) };
    let u64r = |r: &mut dyn Read| -> Result<u64> { Ok(u64::from_le_bytes(rd(r, 8)?.try_into().unwrap())) };

    let version = u32r(r)?;
    if version != VERSION {
        return Err(AppError::NotSupported(format!("snapshot version {version}")));
    }
    let meta_len = u32r(r)? as usize;
    if meta_len > MAX_META {
        return Err(AppError::Corrupt("snapshot metadata too large".into()));
    }
    let meta: ScanMeta =
        serde_json::from_slice(&rd(r, meta_len)?).map_err(|e| AppError::Corrupt(format!("metadata: {e}")))?;
    let count = u64r(r)?;
    if count == 0 || count > u32::MAX as u64 {
        return Err(AppError::Corrupt("invalid node count".into()));
    }
    let count = count as usize;
    let mut nodes = Vec::with_capacity(count.min(1 << 24));
    let mut rec = [0u8; NODE_BYTES];
    for _ in 0..count {
        r.read_exact(&mut rec).map_err(|e| AppError::Corrupt(format!("truncated snapshot: {e}")))?;
        nodes.push(decode_node(&rec));
    }
    let names_len = u64r(r)? as usize;
    let names = rd(r, names_len)?;
    let children_len = u64r(r)? as usize;
    if children_len != count - 1 {
        return Err(AppError::Corrupt("child index size mismatch".into()));
    }
    let raw = rd(r, children_len * 4)?;
    let children: Vec<u32> = raw.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let ext_count = u32r(r)? as usize;
    if ext_count == 0 || ext_count > u16::MAX as usize + 1 {
        return Err(AppError::Corrupt("invalid extension table".into()));
    }
    let mut exts = Vec::with_capacity(ext_count);
    for _ in 0..ext_count {
        let l = rd(r, 1)?[0] as usize;
        let b = rd(r, l)?;
        exts.push(String::from_utf8(b).map_err(|_| AppError::Corrupt("extension not UTF-8".into()))?);
    }

    // Structural validation.
    for (i, n) in nodes.iter().enumerate() {
        let name_end = n.name_off as usize + n.name_len as usize;
        let ok = name_end <= names.len()
            && std::str::from_utf8(&names[n.name_off as usize..name_end]).is_ok()
            && (n.ext as usize) < exts.len()
            && (n.child_start as usize + n.child_count as usize) <= children.len()
            && if i == 0 { true } else { (n.parent as usize) < i };
        if !ok {
            return Err(AppError::Corrupt(format!("invalid node {i}")));
        }
    }
    for &c in &children {
        if c == 0 || c as usize >= count {
            return Err(AppError::Corrupt("invalid child reference".into()));
        }
    }
    Ok(ScanTree::from_parts(meta, nodes, names, children, exts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::builder::{EntryInfo, TreeBuilder};
    use crate::scan::model::ROOT;

    fn sample() -> ScanTree {
        let mut b = TreeBuilder::new("C:\\", 4);
        let d = b.add(ROOT, "dir ção", &EntryInfo { is_dir: true, ..Default::default() });
        b.add(d, "x.mp4", &EntryInfo { size: 1234, alloc: 4096, modified: 99, ..Default::default() });
        b.add(ROOT, "y.zip", &EntryInfo { size: 10, alloc: 4096, ..Default::default() });
        b.finish(ScanMeta { root_path: "C:\\".into(), method: "standard".into(), ..Default::default() })
    }

    #[test]
    fn roundtrip() {
        let t = sample();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.hdcs");
        save(&t, &p).unwrap();
        let l = load(&p).unwrap();
        assert_eq!(l.len(), t.len());
        assert_eq!(l.meta.total_size, t.meta.total_size);
        let x = l.find_path("C:\\dir ção\\x.mp4").unwrap();
        assert_eq!(l.node(x).size, 1234);
        assert_eq!(l.ext(x), "mp4");
    }

    #[test]
    fn rejects_corruption() {
        let t = sample();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.hdcs");
        save(&t, &p).unwrap();
        let mut bytes = std::fs::read(&p).unwrap();
        let len = bytes.len();
        bytes.truncate(len - 10);
        std::fs::write(&p, &bytes).unwrap();
        assert!(load(&p).is_err());
        std::fs::write(&p, b"NOTASNAPSHOT").unwrap();
        assert!(matches!(load(&p), Err(AppError::Corrupt(_))));
    }
}
