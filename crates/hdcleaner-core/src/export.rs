//! Export of scan results (CSV / JSON), streamed so millions of rows never
//! need to be materialised as strings at once.

use crate::format::csv_field;
use crate::scan::model::{NodeId, ScanTree};
use crate::util::filetime_to_unix_ms;
use serde::Deserialize;
use std::io::Write;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Csv,
    Json,
}

impl ExportFormat {
    pub fn parse(s: &str) -> Option<ExportFormat> {
        match s.to_ascii_lowercase().as_str() {
            "csv" => Some(ExportFormat::Csv),
            "json" => Some(ExportFormat::Json),
            _ => None,
        }
    }
}

/// Which nodes to export: an explicit list (e.g. search results) or every
/// node below `root` (depth-first, parents before children).
pub enum ExportSet<'a> {
    Ids(&'a [NodeId]),
    Subtree(NodeId),
}

fn visit(tree: &ScanTree, set: &ExportSet, mut f: impl FnMut(NodeId) -> std::io::Result<()>) -> std::io::Result<()> {
    match set {
        ExportSet::Ids(ids) => {
            for &id in *ids {
                f(id)?;
            }
        }
        ExportSet::Subtree(root) => {
            let mut stack = vec![*root];
            while let Some(id) = stack.pop() {
                f(id)?;
                stack.extend(tree.children(id).iter().rev());
            }
        }
    }
    Ok(())
}

const HEADER: &str = "path,name,type,size,allocated,extension,created,modified,accessed,attributes,files,folders,hardlinkDuplicate";

pub fn write_csv(tree: &ScanTree, set: &ExportSet, w: &mut impl Write) -> std::io::Result<u64> {
    writeln!(w, "{HEADER}")?;
    let mut rows = 0u64;
    visit(tree, set, |id| {
        let n = tree.node(id);
        writeln!(
            w,
            "{},{},{},{},{},{},{},{},{},{},{},{},{}",
            csv_field(&tree.path(id)),
            csv_field(tree.name(id)),
            if n.is_dir() { "dir" } else { "file" },
            n.size,
            n.alloc,
            csv_field(tree.ext(id)),
            filetime_to_unix_ms(n.created),
            filetime_to_unix_ms(n.modified),
            filetime_to_unix_ms(n.accessed),
            n.attributes,
            n.files,
            n.dirs,
            !n.counts_toward_totals()
        )?;
        rows += 1;
        Ok(())
    })?;
    Ok(rows)
}

pub fn write_json(tree: &ScanTree, set: &ExportSet, w: &mut impl Write) -> std::io::Result<u64> {
    let meta = serde_json::to_string(&tree.meta).map_err(std::io::Error::other)?;
    write!(w, "{{\"meta\":{meta},\"items\":[")?;
    let mut rows = 0u64;
    visit(tree, set, |id| {
        let n = tree.node(id);
        let item = serde_json::json!({
            "path": tree.path(id),
            "name": tree.name(id),
            "type": if n.is_dir() { "dir" } else { "file" },
            "size": n.size,
            "allocated": n.alloc,
            "extension": tree.ext(id),
            "created": filetime_to_unix_ms(n.created),
            "modified": filetime_to_unix_ms(n.modified),
            "accessed": filetime_to_unix_ms(n.accessed),
            "attributes": n.attributes,
            "files": n.files,
            "folders": n.dirs,
            "hardlinkDuplicate": !n.counts_toward_totals(),
        });
        if rows > 0 {
            w.write_all(b",")?;
        }
        serde_json::to_writer(&mut *w, &item).map_err(std::io::Error::other)?;
        rows += 1;
        Ok(())
    })?;
    write!(w, "]}}")?;
    Ok(rows)
}

pub fn export_to_file(tree: &ScanTree, set: &ExportSet, format: ExportFormat, path: &std::path::Path) -> crate::Result<u64> {
    let tmp = path.with_extension("partial");
    let f = std::fs::File::create(&tmp).map_err(|e| crate::AppError::io("creating export file", Some(&tmp), e))?;
    let mut w = std::io::BufWriter::with_capacity(1 << 20, f);
    let rows = match format {
        ExportFormat::Csv => {
            // UTF-8 BOM so Excel detects the encoding.
            w.write_all(b"\xEF\xBB\xBF").and_then(|_| write_csv(tree, set, &mut w))
        }
        ExportFormat::Json => write_json(tree, set, &mut w),
    }
    .and_then(|r| w.flush().map(|_| r))
    .map_err(|e| crate::AppError::io("writing export", Some(path), e))?;
    drop(w);
    std::fs::rename(&tmp, path).map_err(|e| crate::AppError::io("finalizing export", Some(path), e))?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::builder::{EntryInfo, TreeBuilder};
    use crate::scan::model::{ScanMeta, ROOT};

    #[test]
    fn csv_and_json() {
        let mut b = TreeBuilder::new("C:\\", 4);
        let d = b.add(ROOT, "a,b", &EntryInfo { is_dir: true, ..Default::default() });
        b.add(d, "=x.txt", &EntryInfo { size: 5, alloc: 4096, ..Default::default() });
        let t = b.finish(ScanMeta { root_path: "C:\\".into(), ..Default::default() });
        let mut out = Vec::new();
        assert_eq!(write_csv(&t, &ExportSet::Subtree(ROOT), &mut out).unwrap(), 3);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("\"C:\\a,b\""));
        assert!(s.contains(",'=x.txt,"));
        let mut out = Vec::new();
        write_json(&t, &ExportSet::Subtree(ROOT), &mut out).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["items"].as_array().unwrap().len(), 3);
        assert_eq!(v["items"][2]["size"], 5);
    }
}
