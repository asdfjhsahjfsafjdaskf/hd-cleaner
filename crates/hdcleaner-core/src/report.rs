//! Storage report: one self-contained HTML page describing a scan.
//!
//! No scripts and no external files — it opens in any browser, can be emailed
//! or archived, and shows only what the scan really measured.

use crate::format::{bytes, datetime};
use crate::scan::model::{NodeId, ScanTree, ROOT};
use crate::stats::{self, ScanStats};
use std::io::Write;

/// How many rows each "largest" table holds.
const TOP: usize = 25;

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64) * 100.0
    }
}

/// The biggest folders and files below `scope`, by allocated size.
fn largest(tree: &ScanTree, scope: NodeId, dirs: bool) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = Vec::new();
    let mut stack = vec![scope];
    while let Some(id) = stack.pop() {
        for &c in tree.children(id) {
            let n = tree.node(c);
            if n.is_dir() {
                stack.push(c);
                if dirs {
                    out.push(c);
                }
            } else if !dirs && n.counts_toward_totals() {
                out.push(c);
            }
        }
    }
    out.sort_by_key(|&id| std::cmp::Reverse(tree.node(id).alloc));
    out.truncate(TOP);
    out
}

fn bar(w: &mut impl Write, label: &str, size: u64, files: u64, whole: u64) -> std::io::Result<()> {
    let p = pct(size, whole);
    write!(
        w,
        "<tr><td class=\"k\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td>\
         <td class=\"b\"><span style=\"width:{:.2}%\"></span></td><td class=\"n p\">{:.1}%</td></tr>",
        esc(label),
        bytes(size),
        files,
        p.clamp(0.0, 100.0),
        p
    )
}

const STYLE: &str = "\
:root{color-scheme:light dark;--fg:#16181d;--muted:#5b6270;--bg:#fbfbfd;--card:#fff;--line:#e4e6ec;--accent:#3b6ef5}\
@media(prefers-color-scheme:dark){:root{--fg:#e7e9ee;--muted:#9aa2b1;--bg:#14161a;--card:#1b1e24;--line:#2a2e37;--accent:#6b93ff}}\
*{box-sizing:border-box}\
body{margin:0;padding:2rem 1.25rem 4rem;background:var(--bg);color:var(--fg);\
font:15px/1.5 -apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif}\
.wrap{max-width:60rem;margin:0 auto}\
h1{font-size:1.5rem;margin:0 0 .25rem}h2{font-size:1.05rem;margin:2rem 0 .6rem}\
.sub{color:var(--muted);margin:0 0 1.5rem;font-size:.92rem}\
.cards{display:grid;grid-template-columns:repeat(auto-fit,minmax(9rem,1fr));gap:.75rem}\
.card{background:var(--card);border:1px solid var(--line);border-radius:.6rem;padding:.75rem .9rem}\
.card b{display:block;font-size:1.25rem;font-weight:600}\
.card span{color:var(--muted);font-size:.85rem}\
table{width:100%;border-collapse:collapse;background:var(--card);border:1px solid var(--line);border-radius:.6rem;overflow:hidden}\
td,th{padding:.4rem .6rem;border-bottom:1px solid var(--line);text-align:left;font-size:.9rem}\
th{color:var(--muted);font-weight:500}tr:last-child td{border-bottom:none}\
.n{text-align:right;white-space:nowrap}.p{color:var(--muted);width:4rem}\
.k{max-width:26rem;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}\
.b{width:30%}.b span{display:block;height:.55rem;border-radius:999px;background:var(--accent);min-width:1px}\
.path{font-family:ui-monospace,Consolas,monospace;font-size:.82rem}\
footer{color:var(--muted);font-size:.82rem;margin-top:2.5rem}";

/// Write the report for `scope` (the whole scan when it is the root).
pub fn write_html(tree: &ScanTree, scope: NodeId, w: &mut impl Write) -> std::io::Result<()> {
    let m = &tree.meta;
    let root = tree.node(scope);
    let title = if scope == ROOT { m.root_path.clone() } else { tree.path(scope) };
    let s: ScanStats = stats::compute(tree, scope, 15);
    let whole = root.alloc.max(1);

    write!(
        w,
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>Storage report — {}</title><style>{STYLE}</style></head><body><div class=\"wrap\">",
        esc(&title)
    )?;
    write!(
        w,
        "<h1>Storage report</h1><p class=\"sub\">{} · scanned {} in {:.1} s ({}, {})</p>",
        esc(&title),
        esc(&datetime(m.started_ms)),
        m.duration_ms as f64 / 1000.0,
        esc(if m.method == "ntfsMft" { "MFT read" } else { "standard scan" }),
        esc(&m.file_system)
    )?;

    // Totals.
    write!(w, "<div class=\"cards\">")?;
    for (value, label) in [
        (bytes(root.size), "size"),
        (bytes(root.alloc), "on disk"),
        (root.files.to_string(), "files"),
        (root.dirs.to_string(), "folders"),
    ] {
        write!(w, "<div class=\"card\"><b>{}</b><span>{label}</span></div>", esc(&value))?;
    }
    if m.volume_total > 0 {
        write!(
            w,
            "<div class=\"card\"><b>{}</b><span>free of {} on the volume</span></div>",
            esc(&bytes(m.volume_free)),
            esc(&bytes(m.volume_total))
        )?;
    }
    write!(w, "</div>")?;

    // By category, by extension, by size.
    for (heading, buckets) in [("By category", &s.categories), ("Top extensions", &s.extensions), ("By file size", &s.histogram)] {
        if buckets.is_empty() {
            continue;
        }
        write!(w, "<h2>{heading}</h2><table><tr><th>name</th><th class=\"n\">on disk</th><th class=\"n\">files</th><th colspan=\"2\"></th></tr>")?;
        for b in buckets.iter().filter(|b| b.files > 0) {
            let key = if b.key.is_empty() { "(no extension)" } else { b.key.as_str() };
            bar(w, key, b.alloc, b.files, whole)?;
        }
        write!(w, "</table>")?;
    }

    // Largest folders and files.
    for (heading, dirs) in [("Largest folders", true), ("Largest files", false)] {
        let items = largest(tree, scope, dirs);
        if items.is_empty() {
            continue;
        }
        write!(w, "<h2>{heading}</h2><table><tr><th>path</th><th class=\"n\">on disk</th><th class=\"n\">{}</th><th colspan=\"2\"></th></tr>", if dirs { "files" } else { "modified" })?;
        for id in items {
            let n = tree.node(id);
            let right = if dirs {
                n.files.to_string()
            } else {
                datetime(crate::util::filetime_to_unix_ms(n.modified))
            };
            let p = pct(n.alloc, whole);
            write!(
                w,
                "<tr><td class=\"k path\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td>\
                 <td class=\"b\"><span style=\"width:{:.2}%\"></span></td><td class=\"n p\">{:.1}%</td></tr>",
                esc(&tree.path(id)),
                esc(&bytes(n.alloc)),
                esc(&right),
                p.clamp(0.0, 100.0),
                p
            )?;
        }
        write!(w, "</table>")?;
    }

    if m.error_count > 0 {
        write!(w, "<h2>Not measured</h2><p class=\"sub\">{} items could not be read (access denied or in use).</p>", m.error_count)?;
    }
    write!(
        w,
        "<footer>Percentages are of the {} shown above. Generated by {} — this file is a report, nothing here changes your disk.</footer>\
         </div></body></html>",
        esc(&bytes(root.alloc)),
        crate::branding::APP_NAME
    )?;
    Ok(())
}

pub fn write_html_file(tree: &ScanTree, scope: NodeId, path: &std::path::Path) -> crate::Result<u64> {
    let tmp = path.with_extension("partial");
    let f = std::fs::File::create(&tmp).map_err(|e| crate::AppError::io("creating the report", Some(&tmp), e))?;
    let mut w = std::io::BufWriter::with_capacity(1 << 16, f);
    write_html(tree, scope, &mut w)
        .and_then(|_| w.flush())
        .map_err(|e| crate::AppError::io("writing the report", Some(path), e))?;
    drop(w);
    let size = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    std::fs::rename(&tmp, path).map_err(|e| crate::AppError::io("finalizing the report", Some(path), e))?;
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::builder::{EntryInfo, TreeBuilder};
    use crate::scan::model::ScanMeta;

    #[test]
    fn writes_a_self_contained_page() {
        let mut b = TreeBuilder::new("C:\\", 8);
        let dir = b.add(ROOT, "Games <x>", &EntryInfo { is_dir: true, ..Default::default() });
        b.add(dir, "save\"1.dat", &EntryInfo { size: 5 << 20, alloc: 5 << 20, ..Default::default() });
        b.add(ROOT, "movie.mp4", &EntryInfo { size: 1 << 30, alloc: 1 << 30, ..Default::default() });
        let tree = b.finish(ScanMeta {
            root_path: "C:\\".into(),
            method: "ntfsMft".into(),
            file_system: "NTFS".into(),
            ..Default::default()
        });

        let mut out = Vec::new();
        write_html(&tree, ROOT, &mut out).unwrap();
        let html = String::from_utf8(out).unwrap();

        assert!(html.starts_with("<!doctype html>") && html.ends_with("</html>"));
        // Nothing is loaded from outside the file.
        assert!(!html.contains("<script"), "no scripts");
        assert!(!html.contains("http://") && !html.contains("https://"), "no external references");
        // Names are escaped, never injected raw.
        assert!(!html.contains("Games <x>"));
        assert!(html.contains("Games &lt;x&gt;"));
        assert!(html.contains("save&quot;1.dat"));
        // The real numbers are there.
        assert!(html.contains("1.00 GB"), "{html}");
        assert!(html.contains("Largest files"));
    }
}
