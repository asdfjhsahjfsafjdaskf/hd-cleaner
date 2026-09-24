//! Search query language over a [`ScanTree`].
//!
//! ```text
//! discord                 name contains "discord"
//! *.mp4  cache*  *.log    wildcard on the name
//! size:>1GB  size:500MB..5GB  size:<=10k
//! modified:<30d  modified:>1y  modified:today  modified:2024-01-01..2024-06-30
//! created:… accessed:…    same syntax as modified
//! ext:mp4  extension:mp4,mkv
//! path:"AppData"          full path contains
//! type:file | type:dir | type:video (any category)
//! -term / !term           negation
//! ```
//! All terms are AND-ed. Matching is case-insensitive.

use crate::category::Category;
use crate::scan::model::{NodeId, ScanTree, ROOT};
use crate::util::{filetime_to_unix_ms, now_unix_ms};
use crate::{AppError, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
enum Cmp {
    Range(u64, u64), // inclusive
}

#[derive(Debug, Clone, PartialEq)]
enum TimeField {
    Modified,
    Created,
    Accessed,
}

#[derive(Debug, Clone, PartialEq)]
enum Term {
    Name(String),
    Wildcard(Vec<char>),
    Size(Cmp),
    Time(TimeField, i64, i64), // unix ms inclusive range
    Ext(Vec<String>),
    Path(String),
    IsDir(bool),
    Category(Category),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    terms: Vec<(bool, Term)>, // (negated, term)
    /// `type:dir` present: size filters then also apply to folder totals.
    dirs_requested: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    Name,
    #[default]
    Size,
    Alloc,
    Modified,
    Created,
    Accessed,
    Ext,
    Path,
    Files,
}

fn tokenize(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in q.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// "1.5GB" -> bytes (binary units).
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let idx = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
    let (num, unit) = s.split_at(idx);
    let v: f64 = num.parse().ok()?;
    let mul: f64 = match unit.trim() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0f64.powi(3),
        "t" | "tb" | "tib" => 1024.0f64.powi(4),
        _ => return None,
    };
    // NaN counts as invalid too: `v < 0.0` alone would let it through.
    if v.is_nan() || v < 0.0 {
        return None;
    }
    Some((v * mul) as u64)
}

fn parse_size_cmp(v: &str) -> Option<Cmp> {
    if let Some((a, b)) = v.split_once("..") {
        let lo = if a.is_empty() { 0 } else { parse_size(a)? };
        let hi = if b.is_empty() { u64::MAX } else { parse_size(b)? };
        return Some(Cmp::Range(lo, hi));
    }
    let (op, rest) = split_op(v);
    let n = parse_size(rest)?;
    Some(match op {
        ">" => Cmp::Range(n.saturating_add(1), u64::MAX),
        ">=" | "" => Cmp::Range(n, u64::MAX),
        "<" => Cmp::Range(0, n.saturating_sub(1)),
        "<=" => Cmp::Range(0, n),
        "=" => Cmp::Range(n, n),
        _ => return None,
    })
}

fn split_op(v: &str) -> (&str, &str) {
    for op in [">=", "<=", ">", "<", "="] {
        if let Some(r) = v.strip_prefix(op) {
            return (op, r);
        }
    }
    ("", v)
}

/// Relative age like "30d" in milliseconds.
fn parse_age(s: &str) -> Option<i64> {
    let s = s.trim().to_ascii_lowercase();
    let idx = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(idx);
    let n: i64 = num.parse().ok()?;
    let ms = match unit {
        "min" => 60_000,
        "h" => 3_600_000,
        "d" | "" => 86_400_000,
        "w" => 7 * 86_400_000,
        "m" | "mo" => 30 * 86_400_000,
        "y" => 365 * 86_400_000,
        _ => return None,
    };
    n.checked_mul(ms)
}

/// "2024-01-31" -> unix ms at local midnight approximated as UTC midnight.
fn parse_date(s: &str) -> Option<i64> {
    let mut it = s.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1601..=9999).contains(&y) {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400_000)
}

fn parse_time_cmp(v: &str, now: i64) -> Option<(i64, i64)> {
    let lower = v.to_ascii_lowercase();
    let day = 86_400_000;
    let today_start = now - now.rem_euclid(day);
    match lower.as_str() {
        "today" => return Some((today_start, i64::MAX)),
        "yesterday" => return Some((today_start - day, today_start - 1)),
        "week" | "thisweek" => return Some((now - 7 * day, i64::MAX)),
        "month" | "thismonth" => return Some((now - 30 * day, i64::MAX)),
        "year" | "thisyear" => return Some((now - 365 * day, i64::MAX)),
        _ => {}
    }
    if let Some((a, b)) = v.split_once("..") {
        let lo = if a.is_empty() { i64::MIN } else { parse_date(a)? };
        let hi = if b.is_empty() { i64::MAX } else { parse_date(b)? + day - 1 };
        return Some((lo, hi));
    }
    let (op, rest) = split_op(v);
    if let Some(date) = parse_date(rest) {
        return Some(match op {
            ">" => (date + day, i64::MAX),
            ">=" => (date, i64::MAX),
            "<" => (i64::MIN, date - 1),
            "<=" => (i64::MIN, date + day - 1),
            "" | "=" => (date, date + day - 1),
            _ => return None,
        });
    }
    let age = parse_age(rest)?;
    // "<30d" = younger than 30 days = modified after now-30d.
    Some(match op {
        "<" | "<=" | "" => (now - age, i64::MAX),
        ">" | ">=" => (i64::MIN, now - age),
        _ => return None,
    })
}

impl Query {
    pub fn parse(q: &str) -> Result<Query> {
        Self::parse_at(q, now_unix_ms())
    }

    pub fn parse_at(q: &str, now: i64) -> Result<Query> {
        let mut terms = Vec::new();
        for tok in tokenize(q) {
            let (neg, tok) = match tok.strip_prefix('-').or_else(|| tok.strip_prefix('!')) {
                Some(rest) if !rest.is_empty() => (true, rest.to_string()),
                _ => (false, tok),
            };
            let bad = |what: &str| AppError::InvalidInput(format!("invalid {what} filter: {tok}"));
            let term = if let Some((key, val)) = tok.split_once(':').filter(|(k, _)| {
                matches!(
                    k.to_ascii_lowercase().as_str(),
                    "size" | "modified" | "created" | "accessed" | "ext" | "extension" | "path" | "type" | "name"
                        | "category" | "cat"
                )
            }) {
                let val = val.trim_matches('"');
                match key.to_ascii_lowercase().as_str() {
                    "size" => Term::Size(parse_size_cmp(val).ok_or_else(|| bad("size"))?),
                    "modified" | "created" | "accessed" => {
                        let (lo, hi) = parse_time_cmp(val, now).ok_or_else(|| bad("date"))?;
                        let f = match key.to_ascii_lowercase().as_str() {
                            "modified" => TimeField::Modified,
                            "created" => TimeField::Created,
                            _ => TimeField::Accessed,
                        };
                        Term::Time(f, lo, hi)
                    }
                    "ext" | "extension" => Term::Ext(
                        val.split(',')
                            .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
                            .filter(|e| !e.is_empty())
                            .collect(),
                    ),
                    "path" => Term::Path(val.to_lowercase().replace('/', "\\")),
                    "name" => name_term(val),
                    _ => match val.to_ascii_lowercase().as_str() {
                        "file" | "files" => Term::IsDir(false),
                        "dir" | "dirs" | "folder" | "folders" | "directory" => Term::IsDir(true),
                        other => Term::Category(Category::parse(other).ok_or_else(|| bad("type"))?),
                    },
                }
            } else {
                name_term(&tok)
            };
            terms.push((neg, term));
        }
        let dirs_requested = terms.iter().any(|(neg, t)| !neg && *t == Term::IsDir(true));
        Ok(Query { terms, dirs_requested })
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    fn needs_paths(&self) -> Vec<&str> {
        self.terms
            .iter()
            .filter_map(|(_, t)| if let Term::Path(p) = t { Some(p.as_str()) } else { None })
            .collect()
    }
}

fn name_term(s: &str) -> Term {
    if s.contains('*') || s.contains('?') {
        Term::Wildcard(s.to_lowercase().chars().collect())
    } else {
        Term::Name(s.to_lowercase())
    }
}

fn contains_ci(hay: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if hay.is_ascii() && needle_lower.is_ascii() {
        let h = hay.as_bytes();
        let n = needle_lower.as_bytes();
        if n.len() > h.len() {
            return false;
        }
        return h.windows(n.len()).any(|w| w.iter().zip(n).all(|(a, b)| a.to_ascii_lowercase() == *b));
    }
    hay.to_lowercase().contains(needle_lower)
}

/// Case-insensitive glob: `*` any run, `?` one char.
pub fn wildcard_match(pattern: &[char], name: &str) -> bool {
    let text: Vec<char> = name.chars().flat_map(|c| c.to_lowercase()).collect();
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = p;
            mark = t;
            p += 1;
        } else if star != usize::MAX {
            p = star + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

/// For each path filter, compute which nodes have a matching full path.
fn path_matches(tree: &ScanTree, needle: &str) -> Vec<bool> {
    let n = tree.len();
    let mut m = vec![false; n];
    let root_l = tree.meta.root_path.to_lowercase();
    m[0] = root_l.contains(needle);
    // Nodes are stored parent-before-child, so a single forward pass works.
    // The match can span the parent/child boundary, so check the tail made of
    // the last `needle.len()` chars of the parent path + '\' + name.
    let keep = needle.chars().count();
    let mut tail_of: Vec<Option<Box<str>>> = vec![None; n];
    tail_of[0] = Some(last_chars(&root_l, keep).into());
    for i in 1..n {
        let node = tree.node(i as NodeId);
        let p = node.parent as usize;
        if m[p] {
            m[i] = true;
            if node.is_dir() {
                tail_of[i] = Some("".into());
            }
            continue;
        }
        let parent_tail = tail_of[p].as_deref().unwrap_or("");
        let mut s = String::with_capacity(parent_tail.len() + 1 + node.name_len as usize);
        s.push_str(parent_tail);
        if !s.ends_with('\\') {
            s.push('\\');
        }
        s.push_str(&tree.name(i as NodeId).to_lowercase());
        m[i] = s.contains(needle);
        if node.is_dir() && !m[i] {
            tail_of[i] = Some(last_chars(&s, keep).into());
        }
    }
    m
}

fn last_chars(s: &str, n: usize) -> &str {
    let count = s.chars().count();
    if count <= n {
        return s;
    }
    let skip = count - n;
    let idx = s.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(0);
    &s[idx..]
}

pub struct SearchResult {
    pub ids: Vec<NodeId>,
    pub total_size: u64,
    pub total_alloc: u64,
}

/// Run `query` against nodes inside `scope` (inclusive of descendants only).
/// `limit` keeps the top-N by the sort key (0 = unlimited).
pub fn search(
    tree: &ScanTree,
    query: &Query,
    scope: NodeId,
    sort: SortKey,
    descending: bool,
    limit: usize,
) -> SearchResult {
    let path_sets: Vec<(String, Vec<bool>)> =
        query.needs_paths().into_iter().map(|p| (p.to_string(), path_matches(tree, p))).collect();
    let in_scope: Option<Vec<bool>> = (scope != ROOT).then(|| {
        let mut v = vec![false; tree.len()];
        v[scope as usize] = true;
        for i in (scope as usize + 1)..tree.len() {
            let p = tree.node(i as NodeId).parent as usize;
            if p != u32::MAX as usize && v[p] {
                v[i] = true;
            }
        }
        v
    });

    let mut ids: Vec<NodeId> = (1..tree.len() as NodeId)
        .into_par_iter()
        .filter(|&id| {
            if tree.is_detached(id) {
                return false;
            }
            if let Some(s) = &in_scope {
                if !s[id as usize] || id == scope {
                    return false;
                }
            }
            query.terms.iter().all(|(neg, t)| eval(tree, id, t, &path_sets, query.dirs_requested) != *neg)
        })
        .collect();

    sort_ids(tree, &mut ids, sort, descending, limit);
    let (total_size, total_alloc) = ids.iter().fold((0u64, 0u64), |(s, a), &id| {
        let n = tree.node(id);
        if n.counts_toward_totals() && !has_ancestor_in(tree, id, &ids) {
            (s + n.size, a + n.alloc)
        } else {
            (s, a)
        }
    });
    SearchResult { ids, total_size, total_alloc }
}

/// Directories and their descendants can both match; avoid double counting
/// in totals only for small result sets (cheap check), otherwise count files.
fn has_ancestor_in(tree: &ScanTree, id: NodeId, ids: &[NodeId]) -> bool {
    if ids.len() > 5000 {
        return tree.node(id).is_dir();
    }
    let mut cur = tree.node(id).parent;
    while cur != u32::MAX {
        if ids.contains(&cur) {
            return true;
        }
        if cur == ROOT {
            break;
        }
        cur = tree.node(cur).parent;
    }
    false
}

fn eval(tree: &ScanTree, id: NodeId, t: &Term, paths: &[(String, Vec<bool>)], dirs_requested: bool) -> bool {
    let n = tree.node(id);
    match t {
        Term::Name(s) => contains_ci(tree.name(id), s),
        Term::Wildcard(p) => wildcard_match(p, tree.name(id)),
        // Folder totals only match when folders were asked for explicitly.
        Term::Size(Cmp::Range(lo, hi)) => (!n.is_dir() || dirs_requested) && n.size >= *lo && n.size <= *hi,
        Term::Time(f, lo, hi) => {
            let ft = match f {
                TimeField::Modified => n.modified,
                TimeField::Created => n.created,
                TimeField::Accessed => n.accessed,
            };
            if ft == 0 {
                return false;
            }
            let ms = filetime_to_unix_ms(ft);
            ms >= *lo && ms <= *hi
        }
        Term::Ext(list) => !n.is_dir() && list.iter().any(|e| e == tree.ext(id)),
        Term::Path(p) => paths.iter().find(|(k, _)| k == p).map(|(_, m)| m[id as usize]).unwrap_or(false),
        Term::IsDir(d) => n.is_dir() == *d,
        Term::Category(c) => !n.is_dir() && tree.category(id) == *c,
    }
}

pub fn sort_ids(tree: &ScanTree, ids: &mut Vec<NodeId>, sort: SortKey, descending: bool, limit: usize) {
    use std::cmp::Ordering;
    let cmp = |a: &NodeId, b: &NodeId| -> Ordering {
        let (na, nb) = (tree.node(*a), tree.node(*b));
        let o = match sort {
            SortKey::Name => tree.name(*a).to_lowercase().cmp(&tree.name(*b).to_lowercase()),
            SortKey::Size => na.size.cmp(&nb.size),
            SortKey::Alloc => na.alloc.cmp(&nb.alloc),
            SortKey::Modified => na.modified.cmp(&nb.modified),
            SortKey::Created => na.created.cmp(&nb.created),
            SortKey::Accessed => na.accessed.cmp(&nb.accessed),
            SortKey::Ext => tree.ext(*a).cmp(tree.ext(*b)),
            SortKey::Files => na.files.cmp(&nb.files),
            SortKey::Path => Ordering::Equal,
        };
        let o = if descending { o.reverse() } else { o };
        o.then(a.cmp(b))
    };
    if sort == SortKey::Path {
        ids.par_sort_by_cached_key(|&id| tree.path(id).to_lowercase());
        if descending {
            ids.reverse();
        }
        if limit > 0 {
            ids.truncate(limit);
        }
        return;
    }
    if limit > 0 && ids.len() > limit {
        ids.select_nth_unstable_by(limit - 1, cmp);
        ids.truncate(limit);
    }
    ids.par_sort_unstable_by(cmp);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::builder::{EntryInfo, TreeBuilder};
    use crate::scan::model::ScanMeta;
    use crate::util::unix_ms_to_filetime;

    const NOW: i64 = 1_750_000_000_000;
    const DAY: i64 = 86_400_000;

    fn tree() -> ScanTree {
        let mut b = TreeBuilder::new("C:\\", 8);
        let f = |size: u64, age_days: i64| EntryInfo {
            size,
            alloc: size,
            modified: unix_ms_to_filetime(NOW - age_days * DAY),
            created: unix_ms_to_filetime(NOW - age_days * DAY),
            ..Default::default()
        };
        let users = b.add(ROOT, "Users", &EntryInfo { is_dir: true, ..Default::default() });
        let appdata = b.add(users, "AppData", &EntryInfo { is_dir: true, ..Default::default() });
        let discord = b.add(appdata, "Discord", &EntryInfo { is_dir: true, ..Default::default() });
        b.add(discord, "Cache_Data.bin", &f(3 << 30, 1));
        b.add(discord, "discord.log", &f(1000, 40));
        b.add(users, "movie.MP4", &f(2 << 30, 400));
        b.add(ROOT, "setup.exe", &f(600 << 20, 10));
        b.add(ROOT, "notes.txt", &f(10, 0));
        b.finish(ScanMeta { root_path: "C:\\".into(), ..Default::default() })
    }

    fn names(t: &ScanTree, q: &str) -> Vec<String> {
        let q = Query::parse_at(q, NOW).unwrap();
        let mut v: Vec<String> =
            search(t, &q, ROOT, SortKey::Name, false, 0).ids.iter().map(|&i| t.name(i).to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("1GB"), Some(1 << 30));
        assert_eq!(parse_size("1.5k"), Some(1536));
        assert_eq!(parse_size("abc"), None);
        let t = tree();
        assert_eq!(names(&t, "size:>1GB"), vec!["Cache_Data.bin", "movie.MP4"]);
        assert_eq!(names(&t, "size:500MB..2.5GB"), vec!["movie.MP4", "setup.exe"]);
    }

    #[test]
    fn names_wildcards_ext() {
        let t = tree();
        assert_eq!(names(&t, "discord"), vec!["Discord", "discord.log"]);
        assert_eq!(names(&t, "*.mp4"), vec!["movie.MP4"]);
        assert_eq!(names(&t, "cache*"), vec!["Cache_Data.bin"]);
        assert_eq!(names(&t, "ext:log,txt"), vec!["discord.log", "notes.txt"]);
        assert_eq!(names(&t, "type:video"), vec!["movie.MP4"]);
        assert_eq!(names(&t, "type:dir"), vec!["AppData", "Discord", "Users"]);
        assert_eq!(names(&t, "type:file -*.exe -*.txt size:<1GB"), vec!["discord.log"]);
    }

    #[test]
    fn dates() {
        let t = tree();
        assert_eq!(names(&t, "modified:<30d type:file"), vec!["Cache_Data.bin", "notes.txt", "setup.exe"]);
        assert_eq!(names(&t, "modified:>1y"), vec!["movie.MP4"]);
        assert_eq!(names(&t, "modified:today"), vec!["notes.txt"]);
        assert!(Query::parse("modified:<zz").is_err());
        assert!(parse_date("2024-02-30").is_some());
        assert_eq!(parse_date("1970-01-02"), Some(DAY));
    }

    #[test]
    fn paths_and_combined() {
        let t = tree();
        assert_eq!(names(&t, "path:\"AppData\" type:file"), vec!["Cache_Data.bin", "discord.log"]);
        assert_eq!(names(&t, "path:users\\app type:file"), vec!["Cache_Data.bin", "discord.log"]);
        assert_eq!(names(&t, "path:\"AppData\" size:>1GB"), vec!["Cache_Data.bin"]);
        assert_eq!(names(&t, "type:dir size:>2GB"), vec!["AppData", "Discord", "Users"]);
    }

    #[test]
    fn wildcard() {
        let p: Vec<char> = "a*b?c".chars().collect();
        assert!(wildcard_match(&p, "AxxxbYc"));
        assert!(!wildcard_match(&p, "abc"));
        assert!(wildcard_match(&"*".chars().collect::<Vec<_>>(), ""));
    }

    #[test]
    fn top_n_and_scope() {
        let t = tree();
        let q = Query::parse_at("type:file", NOW).unwrap();
        let r = search(&t, &q, ROOT, SortKey::Size, true, 2);
        assert_eq!(r.ids.iter().map(|&i| t.name(i)).collect::<Vec<_>>(), vec!["Cache_Data.bin", "movie.MP4"]);
        let users = t.find_path("C:\\Users\\AppData").unwrap();
        let r = search(&t, &q, users, SortKey::Size, true, 0);
        assert_eq!(r.ids.len(), 2);
    }
}
