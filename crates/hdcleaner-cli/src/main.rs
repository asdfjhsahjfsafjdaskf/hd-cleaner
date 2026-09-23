//! `hdcleaner` command line interface. Uses the same core services as the GUI.
//! Destructive commands require `--confirm`; `--dry-run` shows what would happen.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use hdcleaner_core::forced::ForcedInput;
use hdcleaner_core::format::{bytes, datetime};
use hdcleaner_core::leftovers::Level;
use hdcleaner_core::programs::Program;
use hdcleaner_core::uninstall::UninstallCommand;
use hdcleaner_core::scan::{ScanControl, ScanMethod, ScanOptions, ScanTree, ROOT};
use hdcleaner_core::search::{Query, SortKey};
use hdcleaner_core::util::filetime_to_unix_ms;
use hdcleaner_core::{branding, AppError};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(name = branding::CLI_NAME, version, about = format!("{} command line", branding::APP_NAME))]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    /// Output machine-readable JSON where supported.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Method {
    Auto,
    Standard,
    Ntfs,
}

#[derive(Subcommand)]
enum Cmd {
    /// List drives with capacity and file system.
    Drives,
    /// Scan a drive or folder and print a summary.
    Scan {
        path: String,
        #[arg(long, value_enum, default_value = "auto")]
        method: Method,
        /// Request administrator rights (UAC) for the NTFS MFT fast scan.
        #[arg(long)]
        elevate: bool,
        /// Number of largest folders to show.
        #[arg(long, default_value_t = 15)]
        top: usize,
        /// Save a snapshot (.hdcs) for later comparison.
        #[arg(long)]
        snapshot: Option<PathBuf>,
    },
    /// Largest files under a path.
    Largest {
        path: String,
        #[arg(long, default_value_t = 100)]
        count: usize,
    },
    /// Search with the query language, e.g. `hdcleaner search C: "size:>1GB ext:iso"`.
    Search {
        path: String,
        query: String,
        #[arg(long, default_value_t = 100)]
        count: usize,
    },
    /// Find duplicate files.
    Duplicates {
        path: String,
        /// quick (name+size) or precise (hash).
        #[arg(long, default_value = "precise")]
        mode: String,
        /// Minimum file size, e.g. 1MB.
        #[arg(long, default_value = "1MB")]
        min_size: String,
    },
    /// Export scan results to CSV or JSON.
    Export {
        path: String,
        #[arg(long, default_value = "csv")]
        format: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Compare two snapshots.
    Diff { old: PathBuf, new: PathBuf, #[arg(long, default_value_t = 30)] top: usize },
    /// Delete files/folders (Recycle Bin by default). Requires --confirm.
    Delete {
        paths: Vec<String>,
        #[arg(long)]
        permanent: bool,
        #[arg(long)]
        confirm: bool,
        #[arg(long)]
        dry_run: bool,
        /// Also allow items classified as Dangerous (e.g. inside Program Files).
        #[arg(long)]
        allow_dangerous: bool,
    },
    /// List installed programs (registry + Store/AppX).
    Programs {
        /// Only programs whose name/publisher contains this text.
        filter: Option<String>,
        /// Include system components, updates and frameworks.
        #[arg(long)]
        all: bool,
        /// Also measure the real size (install folder + AppData/ProgramData).
        #[arg(long)]
        sizes: bool,
    },
    /// Uninstall a program with its official uninstaller, then scan leftovers.
    /// Runs nothing without --confirm; leftovers are removed only with
    /// --remove-leftovers (pre-selected items: high confidence, not shared).
    ///
    /// --forced: for broken or unregistered programs; starts from a name,
    /// an --exe and/or a --folder instead of the installed-programs list.
    Uninstall {
        /// Program name (required unless --forced with --exe or --folder).
        name: Option<String>,
        #[arg(long)]
        quiet: bool,
        #[arg(long)]
        forced: bool,
        /// Forced: the program's executable.
        #[arg(long, requires = "forced")]
        exe: Option<String>,
        /// Forced: the program's folder.
        #[arg(long, requires = "forced")]
        folder: Option<String>,
        /// Forced: target this registered program (exact name from the match list).
        #[arg(long, requires = "forced", conflicts_with = "unregistered")]
        program: Option<String>,
        /// Forced: ignore registered matches (clean only what is found).
        #[arg(long, requires = "forced")]
        unregistered: bool,
        /// Forced: end the processes running from the program folder (needs --confirm).
        #[arg(long, requires = "forced")]
        end_processes: bool,
        /// Forced: run the official uninstaller first when it still exists (needs --confirm).
        #[arg(long, requires = "forced")]
        run_official: bool,
        /// safe | moderate | advanced
        #[arg(long, default_value = "moderate")]
        level: String,
        #[arg(long)]
        remove_leftovers: bool,
        #[arg(long)]
        confirm: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// List what starts with Windows (Run keys, Startup folders, logon/boot tasks, third-party services).
    /// Changing an entry needs --confirm; admin entries ask for UAC once.
    Startup {
        /// Filter by name, command or publisher.
        filter: Option<String>,
        /// Disable the entry with this exact name (reversible, like Task Manager).
        #[arg(long, conflicts_with_all = ["enable", "remove"])]
        disable: Option<String>,
        /// Enable the entry with this exact name.
        #[arg(long, conflicts_with = "remove")]
        enable: Option<String>,
        /// Remove the entry with this exact name (backup first).
        #[arg(long)]
        remove: Option<String>,
        /// Pick one entry by id when several share the name (ids are shown with --json).
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        confirm: bool,
        /// Show the startup delays recorded by Windows (boot performance log; one UAC prompt).
        #[arg(long)]
        impact: bool,
    },
    /// End a process (optionally its tree). Windows components are never ended.
    Kill {
        pid: u32,
        #[arg(long)]
        tree: bool,
        /// Use the elevated helper (processes of other accounts / elevated).
        #[arg(long)]
        elevated: bool,
        #[arg(long)]
        confirm: bool,
    },
    /// List running processes (memory, CPU over one second, user, publisher).
    Processes {
        filter: Option<String>,
        /// Sort by: memory | cpu | name
        #[arg(long, default_value = "memory")]
        sort: String,
        #[arg(long, default_value_t = 40)]
        top: usize,
    },
    /// Analyze junk files (default) or clean chosen categories.
    /// Cleaning needs --run with category ids and --confirm (or --dry-run).
    Cleanup {
        /// Only analyze (the default when --run is not given).
        #[arg(long)]
        analyze: bool,
        /// Comma-separated category ids to clean (see the analysis), or "default".
        #[arg(long)]
        run: Option<String>,
        /// Show the items of one category.
        #[arg(long)]
        items: Option<String>,
        #[arg(long)]
        confirm: bool,
        #[arg(long)]
        dry_run: bool,
    },
}

fn method(m: Method) -> ScanMethod {
    match m {
        Method::Auto => ScanMethod::Auto,
        Method::Standard => ScanMethod::Standard,
        Method::Ntfs => ScanMethod::NtfsMft,
    }
}

fn scan_with_progress(path: &str, m: ScanMethod, elevate: bool, quiet: bool) -> Result<ScanTree> {
    let ctl = Arc::new(ScanControl::new());
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reporter = (!quiet).then(|| {
        let (ctl, done) = (ctl.clone(), done.clone());
        std::thread::spawn(move || {
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                let p = ctl.snapshot();
                eprint!("\r  {:>12} files  {:>10} folders  {:>10}   ", p.files, p.dirs, bytes(p.bytes));
                std::thread::sleep(Duration::from_millis(250));
            }
            eprintln!();
        })
    });
    let root = hdcleaner_core::util::normalize_root(path)?;
    let is_volume = root.len() == 3;
    let result = if elevate && is_volume && !hdcleaner_core::system::is_elevated() {
        hdcleaner_core::elevation::scan_ntfs_elevated(root.chars().next().unwrap(), &ctl)
    } else {
        hdcleaner_core::scan::run_scan(&root, &ScanOptions { method: m, follow_junctions: false }, &ctl)
    };
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(r) = reporter {
        let _ = r.join();
    }
    result.map_err(explain)
}

fn explain(e: AppError) -> anyhow::Error {
    let hint = match e.suggestion() {
        Some("runElevated") => "\n  hint: run from an elevated terminal or pass --elevate",
        Some("rescan") => "\n  hint: the item no longer exists; scan again",
        Some("protectedLocation") => "\n  hint: this location is protected by the Protection Engine",
        _ => "",
    };
    anyhow::anyhow!("{e}{hint}")
}

fn main() {
    if let Some(code) = hdcleaner_core::elevation::maybe_run_helper() {
        std::process::exit(code);
    }
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("HDCLEANER_LOG").unwrap_or_else(|_| "warn".into()))
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.cmd {
        Cmd::Drives => {
            let drives = hdcleaner_core::disk::list_drives();
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&drives)?);
            } else {
                println!("{:<5} {:<8} {:<10} {:<8} {:>12} {:>12} {:>6}  LABEL", "DRIVE", "FS", "TYPE", "MEDIA", "TOTAL", "FREE", "USED");
                for d in drives {
                    let pct = if d.total_bytes > 0 { d.used_bytes * 100 / d.total_bytes } else { 0 };
                    println!(
                        "{:<5} {:<8} {:<10} {:<8} {:>12} {:>12} {:>5}%  {}",
                        d.root,
                        d.file_system,
                        format!("{:?}", d.kind).to_lowercase(),
                        format!("{:?}", d.media).to_lowercase(),
                        bytes(d.total_bytes),
                        bytes(d.free_bytes),
                        pct,
                        d.label
                    );
                }
            }
        }
        Cmd::Scan { path, method: m, elevate, top, snapshot } => {
            let tree = scan_with_progress(&path, method(m), elevate, cli.json)?;
            if let Some(p) = snapshot {
                hdcleaner_core::scan::snapshot::save(&tree, &p).map_err(explain)?;
                eprintln!("snapshot saved to {}", p.display());
            }
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&tree.meta)?);
                return Ok(());
            }
            let m = &tree.meta;
            println!("{}  [{} / {}]", m.root_path, m.method, m.file_system);
            println!(
                "  {} files, {} folders, {} ({} allocated) in {:.2}s",
                m.files,
                m.dirs,
                bytes(m.total_size),
                bytes(m.total_alloc),
                m.duration_ms as f64 / 1000.0
            );
            if m.error_count > 0 {
                println!("  {} folders could not be read (e.g. {})", m.error_count, m.error_samples.first().map(|s| s.path.as_str()).unwrap_or(""));
            }
            for n in &m.notes {
                println!("  note: {n}");
            }
            println!("\n  largest items in {}:", m.root_path);
            let total = tree.node(ROOT).alloc.max(1);
            for &c in tree.children(ROOT).iter().take(top) {
                let n = tree.node(c);
                println!(
                    "  {:>10}  {:>5.1}%  {}{}",
                    bytes(n.alloc),
                    n.alloc as f64 * 100.0 / total as f64,
                    tree.name(c),
                    if n.is_dir() { "\\" } else { "" }
                );
            }
        }
        Cmd::Largest { path, count } => {
            let tree = scan_with_progress(&path, ScanMethod::Auto, false, cli.json)?;
            let q = Query::parse("type:file")?;
            let r = hdcleaner_core::search::search(&tree, &q, ROOT, SortKey::Size, true, count);
            print_rows(&tree, &r.ids, cli.json)?;
        }
        Cmd::Search { path, query, count } => {
            let q = Query::parse(&query).map_err(explain)?;
            let tree = scan_with_progress(&path, ScanMethod::Auto, false, cli.json)?;
            let r = hdcleaner_core::search::search(&tree, &q, ROOT, SortKey::Size, true, 0);
            if !cli.json {
                eprintln!("{} matches, {} total", r.ids.len(), bytes(r.total_size));
            }
            print_rows(&tree, &r.ids[..r.ids.len().min(count)], cli.json)?;
        }
        Cmd::Duplicates { path, mode, min_size } => {
            let mode = match mode.as_str() {
                "quick" => hdcleaner_core::duplicates::DupMode::Quick,
                "precise" => hdcleaner_core::duplicates::DupMode::Precise,
                other => bail!("unknown mode {other} (quick|precise)"),
            };
            let min = hdcleaner_core::search::parse_size(&min_size).context("invalid --min-size")?;
            let tree = scan_with_progress(&path, ScanMethod::Auto, false, cli.json)?;
            let ctl = ScanControl::new();
            let r = hdcleaner_core::duplicates::find_duplicates(
                &tree,
                &hdcleaner_core::duplicates::DupOptions { mode, min_size: min, scope: None },
                &ctl,
            )
            .map_err(explain)?;
            if cli.json {
                let groups: Vec<_> = r
                    .groups
                    .iter()
                    .map(|g| serde_json::json!({"size": g.size, "hash": g.hash, "wasted": g.wasted,
                        "files": g.files.iter().map(|&f| tree.path(f)).collect::<Vec<_>>()}))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({"totalWasted": r.total_wasted, "groups": groups}))?);
            } else {
                println!("{} groups, {} wasted ({} hashed)", r.groups.len(), bytes(r.total_wasted), bytes(r.bytes_hashed));
                for g in r.groups.iter().take(50) {
                    println!("\n  {} x{}  wasted {}", bytes(g.size), g.files.len(), bytes(g.wasted));
                    for &f in &g.files {
                        println!("    {}", tree.path(f));
                    }
                }
            }
        }
        Cmd::Export { path, format, output } => {
            let fmt = hdcleaner_core::export::ExportFormat::parse(&format).context("format must be csv or json")?;
            let tree = scan_with_progress(&path, ScanMethod::Auto, false, cli.json)?;
            let rows = hdcleaner_core::export::export_to_file(&tree, &hdcleaner_core::export::ExportSet::Subtree(ROOT), fmt, &output)
                .map_err(explain)?;
            eprintln!("{rows} rows written to {}", output.display());
        }
        Cmd::Diff { old, new, top } => {
            let a = hdcleaner_core::scan::snapshot::load(&old).map_err(explain)?;
            let b = hdcleaner_core::scan::snapshot::load(&new).map_err(explain)?;
            let r = hdcleaner_core::diff::diff(&a, &b, top);
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                let sign = if r.total_delta >= 0 { "+" } else { "-" };
                println!("total change: {sign}{}", bytes(r.total_delta.unsigned_abs()));
                println!("files: +{} added, -{} removed, {} grew, {} shrank", r.added_files, r.removed_files, r.grown_files, r.shrunk_files);
                println!("\nfolders:");
                for c in r.folders.iter().take(top) {
                    println!("  {}{:>10}  {:?}  {}", if c.delta >= 0 { "+" } else { "-" }, bytes(c.delta.unsigned_abs()), c.kind, c.path);
                }
                println!("\nfiles:");
                for c in r.files.iter().take(top) {
                    println!("  {}{:>10}  {:?}  {}", if c.delta >= 0 { "+" } else { "-" }, bytes(c.delta.unsigned_abs()), c.kind, c.path);
                }
            }
        }
        Cmd::Delete { paths, permanent, confirm, dry_run, allow_dangerous } => {
            use hdcleaner_core::fsops::*;
            if paths.is_empty() {
                bail!("no paths given");
            }
            let mode = if permanent { DeleteMode::Permanent } else { DeleteMode::RecycleBin };
            let items: Vec<(String, Option<u64>)> = paths.iter().map(|p| (p.clone(), None)).collect();
            let plan = plan_delete(&items, mode);
            println!("plan ({:?}):", mode);
            for i in &plan.items {
                println!(
                    "  [{:?}/{}] {}{}",
                    i.assessment.risk,
                    i.assessment.reason,
                    i.path,
                    i.error.as_ref().map(|e| format!("  (error: {})", e.message)).unwrap_or_default()
                );
            }
            if !confirm && !dry_run {
                bail!("nothing deleted: pass --confirm to delete, or --dry-run to simulate");
            }
            let results = execute_delete(&plan, &ExecuteOptions { dry_run, allow_dangerous }, |_, _| {}, || true);
            for r in &results {
                println!("  {:?}  {}", r.outcome, r.path);
            }
        }
        Cmd::Programs { filter, all, sizes } => {
            let (list, appx_err) = hdcleaner_core::programs::list_all();
            if let Some(e) = appx_err {
                eprintln!("warning: Store/AppX packages not listed: {e}");
            }
            let f = filter.map(|s| s.to_lowercase());
            let roots = hdcleaner_core::appsize::Roots::load();
            let mut rows = Vec::new();
            for p in list.iter().filter(|p| all || !(p.system_component || p.is_update || p.is_framework)) {
                let hay = format!("{} {}", p.name, p.publisher.as_deref().unwrap_or("")).to_lowercase();
                if f.as_ref().is_some_and(|f| !hay.contains(f.as_str())) {
                    continue;
                }
                let size = sizes.then(|| {
                    hdcleaner_core::appsize::compute(p, &roots, |path| {
                        hdcleaner_core::appsize::measure_live(path, &ScanControl::new()).map(|m| (m, "live"))
                    })
                });
                rows.push((p, size));
            }
            if cli.json {
                let v: Vec<_> = rows.iter().map(|(p, s)| serde_json::json!({ "program": p, "realSize": s })).collect();
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                for (p, s) in &rows {
                    let real = s.as_ref().map(|s| format!("  real {}", bytes(s.total))).unwrap_or_default();
                    let rep = p.reported_size.map(|r| format!("  reported {}", bytes(r))).unwrap_or_default();
                    println!("{:<50} {:<16} {:?}{rep}{real}", p.name, p.version.as_deref().unwrap_or(""), p.source);
                }
                eprintln!("{} programs", rows.len());
            }
        }
        Cmd::Uninstall { name, quiet, forced, exe, folder, program, unregistered, end_processes, run_official, level, remove_leftovers, confirm, dry_run } => {
            let level = parse_level(&level)?;
            let leftovers = LeftoverStep { level, remove: remove_leftovers, confirm, dry_run };
            if forced {
                return forced_uninstall(ForcedInput { name, exe, folder }, program, unregistered, end_processes, run_official, quiet, &leftovers);
            }
            let Some(name) = name else { bail!("give the program name (or use --forced with --exe/--folder)") };
            let (all, _) = hdcleaner_core::programs::list_all();
            let exact: Vec<_> = all.iter().filter(|p| p.name.eq_ignore_ascii_case(&name)).collect();
            let matches: Vec<_> = if exact.is_empty() {
                all.iter().filter(|p| p.name.to_lowercase().contains(&name.to_lowercase())).collect()
            } else {
                exact
            };
            let prog = match matches.as_slice() {
                [one] => (*one).clone(),
                [] => bail!("no installed program matches \"{name}\""),
                many => {
                    for p in many {
                        eprintln!("  {}  {}", p.name, p.version.as_deref().unwrap_or(""));
                    }
                    bail!("{} programs match; use the exact name", many.len());
                }
            };
            println!("{}  {}  ({:?})", prog.name, prog.version.as_deref().unwrap_or(""), prog.source);
            if hdcleaner_core::uninstall::still_installed(&prog) {
                let cmd = hdcleaner_core::uninstall::command_for(&prog, quiet).map_err(explain)?;
                println!("command: \"{}\" {}", cmd.file, cmd.params);
                if !confirm {
                    bail!("nothing done: pass --confirm to run the uninstaller");
                }
                if run_uninstaller(&prog, &cmd)? {
                    bail!("the program is still installed (uninstaller cancelled?); leftovers are not scanned");
                }
            }
            leftovers.run(&prog, &all)?;
        }
        Cmd::Startup { filter, disable, enable, remove, id, confirm, impact } => {
            if impact {
                let r = hdcleaner_core::bootperf::read_any().map_err(explain)?;
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&r)?);
                    return Ok(());
                }
                println!("recent boots (Windows boot performance log):");
                for b in r.boots.iter().take(8) {
                    println!(
                        "  {}  total {:.1}s  (main path {:.1}s, after desktop {:.1}s)  {} startup apps",
                        datetime(b.time_ms),
                        b.boot_ms as f64 / 1000.0,
                        b.main_path_ms as f64 / 1000.0,
                        b.post_boot_ms as f64 / 1000.0,
                        b.startup_apps
                    );
                }
                println!("\nrecorded startup delays (average / worst, boots):");
                for s in &r.slowdowns {
                    println!("  +{:>5.1}s / {:>5.1}s  x{:<3} {:<8} {:<32} {}", s.avg_delay_ms as f64 / 1000.0, s.max_delay_ms as f64 / 1000.0, s.count, s.kind, s.name, s.path);
                }
                return Ok(());
            }
            let db = hdcleaner_core::db::Database::open(&hdcleaner_core::util::app_data_dir().join("hdcleaner.db")).ok();
            let ours = db.as_ref().map(hdcleaner_core::startup::disabled_services).unwrap_or_default();
            if let Some((name, action)) = disable.map(|n| (n, "disable")).or(enable.map(|n| (n, "enable"))).or(remove.map(|n| (n, "remove"))) {
                return startup_action(&name, action, id.as_deref(), confirm, &ours, db.as_ref());
            }
            let f = filter.map(|s| s.to_lowercase());
            let items: Vec<_> = hdcleaner_core::startup::list(&ours)
                .into_iter()
                .filter(|i| f.as_ref().is_none_or(|f| format!("{} {} {}", i.name, i.command, i.company.as_deref().unwrap_or("")).to_lowercase().contains(f.as_str())))
                .collect();
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else {
                for i in &items {
                    let src = match &i.source {
                        hdcleaner_core::startup::Source::RunKey { hive, once, .. } => format!("{}{}", hive.short(), if *once { " RunOnce" } else { " Run" }),
                        hdcleaner_core::startup::Source::StartupFolder { common } => if *common { "Startup (all)" } else { "Startup" }.to_string(),
                        hdcleaner_core::startup::Source::Task => "Task".to_string(),
                        hdcleaner_core::startup::Source::Service => "Service".to_string(),
                    };
                    let state = if i.enabled { "on " } else { "off" };
                    let flags = [(!i.exe_exists).then_some("missing file"), i.is_windows.then_some("windows"), i.detail.as_deref()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!("[{state}] {:<14} {:<34} {:<26} {}{}", src, i.name, i.company.as_deref().unwrap_or(""), i.command, if flags.is_empty() { String::new() } else { format!("  ({flags})") });
                }
                eprintln!("{} entries", items.len());
            }
        }
        Cmd::Processes { filter, sort, top } => {
            let mut s = hdcleaner_core::processes::ProcessSampler::default();
            s.sample();
            std::thread::sleep(Duration::from_secs(1));
            let f = filter.map(|s| s.to_lowercase());
            let mut rows: Vec<_> = s
                .sample()
                .into_iter()
                .filter(|r| f.as_ref().is_none_or(|f| format!("{} {}", r.name, r.path.as_deref().unwrap_or("")).to_lowercase().contains(f.as_str())))
                .collect();
            match sort.as_str() {
                "cpu" => rows.sort_by(|a, b| b.cpu.unwrap_or(0.0).total_cmp(&a.cpu.unwrap_or(0.0))),
                "name" => rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
                "memory" => rows.sort_by(|a, b| b.private_bytes.cmp(&a.private_bytes)),
                other => bail!("unknown sort {other} (memory|cpu|name)"),
            }
            rows.truncate(top);
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                println!("{:>7} {:>6} {:>10}  {:<28} {:<22} {}", "PID", "CPU%", "MEMORY", "NAME", "USER", "PUBLISHER");
                for r in &rows {
                    println!(
                        "{:>7} {:>6.1} {:>10}  {:<28} {:<22} {}",
                        r.pid,
                        r.cpu.unwrap_or(0.0),
                        r.private_bytes.map(bytes).unwrap_or_default(),
                        r.name,
                        r.user.as_deref().unwrap_or(""),
                        r.company.as_deref().unwrap_or("")
                    );
                }
            }
        }
        Cmd::Kill { pid, tree, elevated, confirm } => {
            let path = hdcleaner_core::processes::image_path(pid);
            if path.is_none() && !elevated {
                bail!("process {pid} not found or not accessible (SYSTEM / another account); add --elevated");
            }
            let kids = if tree { hdcleaner_core::processes::descendants(pid) } else { Vec::new() };
            println!("{pid}  {}", path.as_deref().unwrap_or("(path readable only by an administrator)"));
            for k in &kids {
                println!("  child {}  {}", k.pid, k.path.as_deref().unwrap_or(&k.name));
            }
            if !confirm {
                bail!("nothing done: pass --confirm to end {}", if tree { "these processes" } else { "this process" });
            }
            let r = match (elevated, path) {
                (true, p) => hdcleaner_core::processes::terminate_elevated(pid, p.as_deref(), tree),
                (false, Some(p)) if tree => hdcleaner_core::processes::terminate_tree(pid, &p),
                (false, Some(p)) => hdcleaner_core::processes::terminate(pid, &p).map(|_| {
                    vec![hdcleaner_core::processes::TreeKill { pid, name: p.clone(), path: Some(p.clone()), ok: true, skipped: false, error: None }]
                }),
                (false, None) => unreachable!(),
            };
            match r {
                Ok(list) => {
                    for k in list {
                        let state = if k.ok { "ended" } else if k.skipped { "kept (Windows)" } else { "failed" };
                        println!("  {state:<15} {:>6}  {}{}", k.pid, k.name, k.error.map(|e| format!("  ({})", e.message)).unwrap_or_default());
                    }
                }
                Err(e @ AppError::AccessDenied { .. }) => bail!("{e}\n  hint: the process belongs to another account or is elevated; add --elevated"),
                Err(e) => return Err(explain(e)),
            }
        }
        Cmd::Cleanup { analyze: _, run, items, confirm, dry_run } => {
            use hdcleaner_core::cleaner;
            eprintln!("analyzing...");
            let mut results = cleaner::analyze();
            // C:\Windows\Temp and other machine-wide folders cannot be listed
            // by a standard user: the helper lists them (one UAC prompt).
            let pending: Vec<String> = results
                .iter()
                .filter(|r| r.admin_pending && run.as_deref().is_some_and(|r2| r2 == "default" || r2.split(',').any(|x| x.trim() == r.category.id)))
                .map(|r| r.category.id.clone())
                .collect();
            if !pending.is_empty() {
                eprintln!("listing {} machine-wide categories as administrator...", pending.len());
                let out = hdcleaner_core::elevation::run_elevated_ops(&[hdcleaner_core::elevation::ElevatedOp::AnalyzeClean { categories: pending }]).map_err(explain)?;
                let listed: Vec<cleaner::AdminAnalysis> = out.into_iter().next().and_then(|r| r.data).and_then(|d| serde_json::from_value(d).ok()).unwrap_or_default();
                for a in listed {
                    if let Some(r) = results.iter_mut().find(|r| r.category.id == a.id) {
                        r.count = a.items.len() as u64;
                        r.bytes = a.items.iter().map(|i| i.size).sum();
                        r.items = a.items;
                        r.recent_skipped = a.recent_skipped;
                        r.admin_pending = false;
                    }
                }
            }
            let results = results;
            if let Some(id) = items {
                let r = results.iter().find(|r| r.category.id == id).with_context(|| format!("no category {id}"))?;
                for i in &r.items {
                    println!("{:>10}  {}", bytes(i.size), i.display.as_deref().map(|d| format!("{d}   [{}]", i.path)).unwrap_or_else(|| i.path.clone()));
                }
                return Ok(());
            }
            let Some(run) = run else {
                if cli.json {
                    let v: Vec<_> = results.iter().map(|r| serde_json::json!({"id": r.category.id, "owner": r.category.owner, "risk": r.category.risk,
                        "defaultOn": r.category.default_on, "admin": r.category.admin, "count": r.count, "bytes": r.bytes, "running": r.running,
                        "recentSkipped": r.recent_skipped})).collect();
                    println!("{}", serde_json::to_string_pretty(&v)?);
                    return Ok(());
                }
                let mut total = 0;
                for r in &results {
                    let c = &r.category;
                    let flags = [c.default_on.then_some("default"), c.admin.then_some("admin"), r.running.then_some("RUNNING"), r.admin_pending.then_some("needs admin to list"), r.error.as_ref().map(|_| "error")]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(",");
                    println!("{:<34} {:<9} {:>8} items {:>10}  {:<22} {flags}", c.id, format!("{:?}", c.risk), r.count, bytes(r.bytes), c.owner.as_deref().unwrap_or(""));
                    if c.default_on && !r.running {
                        total += r.bytes;
                    }
                }
                println!("\ndefault selection frees about {}", bytes(total));
                return Ok(());
            };
            let ids: Vec<String> = if run == "default" {
                results.iter().filter(|r| r.category.default_on).map(|r| r.category.id.clone()).collect()
            } else {
                run.split(',').map(|s| s.trim().to_string()).collect()
            };
            if !confirm && !dry_run {
                bail!("nothing removed: pass --confirm to clean (or --dry-run to simulate)");
            }
            let backup = hdcleaner_core::util::app_data_dir().join("backups").join(format!("cli-cleanup-{}", hdcleaner_core::util::now_unix_ms()));
            let mut elevated = Vec::new();
            for id in &ids {
                let r = results.iter().find(|r| &r.category.id == id).with_context(|| format!("no category {id}"))?;
                if r.running {
                    println!("{id:<34} skipped: {} is running", r.category.owner.as_deref().unwrap_or(id));
                    continue;
                }
                if r.category.admin && !hdcleaner_core::system::is_elevated() && !dry_run {
                    elevated.push(hdcleaner_core::elevation::ElevatedOp::Clean { category: id.clone(), items: r.items.clone() });
                    continue;
                }
                match cleaner::clean_category(id, &r.items, &backup, dry_run, &mut |_| {}) {
                    Ok(o) => println!("{id:<34} removed {:>7}  freed {:>10}  in use {}  changed {}  denied {}  failed {}", o.removed, bytes(o.freed), o.in_use, o.changed, o.denied, o.failed),
                    Err(e) => println!("{id:<34} {e}"),
                }
            }
            if !elevated.is_empty() {
                println!("administrator rights needed for {} categories (one UAC prompt)...", elevated.len());
                let names: Vec<String> = elevated.iter().map(|o| match o { hdcleaner_core::elevation::ElevatedOp::Clean { category, .. } => category.clone(), _ => String::new() }).collect();
                for (id, r) in names.iter().zip(hdcleaner_core::elevation::run_elevated_ops(&elevated).map_err(explain)?) {
                    match r.data.and_then(|d| serde_json::from_value::<cleaner::CleanOutcome>(d).ok()) {
                        Some(o) if r.ok => println!("{id:<34} removed {:>7}  freed {:>10}  in use {}  changed {}  denied {}  failed {}", o.removed, bytes(o.freed), o.in_use, o.changed, o.denied, o.failed),
                        _ => println!("{id:<34} {}", r.message.unwrap_or_default()),
                    }
                }
            }
        }
    }
    Ok(())
}

fn parse_level(s: &str) -> Result<Level> {
    Ok(match s {
        "safe" => Level::Safe,
        "moderate" => Level::Moderate,
        "advanced" => Level::Advanced,
        other => bail!("unknown level {other} (safe|moderate|advanced)"),
    })
}

/// Runs an official uninstaller and waits for its process tree.
/// Returns whether the program is still installed afterwards.
fn run_uninstaller(prog: &Program, cmd: &UninstallCommand) -> Result<bool> {
    let stop = std::sync::atomic::AtomicBool::new(false);
    let out = hdcleaner_core::uninstall::run(prog, cmd, &stop, |p| {
        if !p.is_empty() {
            eprint!("\r  waiting: {}        ", p.join(", "));
        }
    })
    .map_err(explain)?;
    eprintln!();
    println!("exit code {:?}, {} ms", out.exit_code, out.duration_ms);
    Ok(out.still_installed)
}

/// Leftover scan, listing and (optionally) removal of the pre-selected items.
struct LeftoverStep {
    level: Level,
    remove: bool,
    confirm: bool,
    dry_run: bool,
}

impl LeftoverStep {
    fn run(&self, prog: &Program, others: &[Program]) -> Result<()> {
        use hdcleaner_core::leftovers::{self, RemovalOptions};
        let (level, dry_run) = (self.level, self.dry_run);
        let found = leftovers::scan(prog, others, level, true);
        println!("\n{} leftovers ({level:?}):", found.len());
        for l in &found {
            println!(
                "  [{}] {:>3}% {:<9} {:<10} {}  ({})",
                if l.preselected { "x" } else { " " },
                l.confidence,
                format!("{:?}", l.level),
                bytes(l.size),
                l.path,
                l.reason
            );
        }
        if !self.remove {
            return Ok(());
        }
        if !self.confirm && !dry_run {
            bail!("leftovers not removed: pass --confirm (or --dry-run)");
        }
        let chosen: Vec<_> = found.into_iter().filter(|l| l.preselected).collect();
        let backup = hdcleaner_core::util::app_data_dir().join("backups").join(format!("cli-{}", hdcleaner_core::util::now_unix_ms()));
        let leftovers::Removal { results, pending, saved } =
            leftovers::remove(&chosen, &RemovalOptions { backup_dir: &backup, recycle: true, allow_dangerous: true, dry_run, quarantine: true });
        if !saved.is_empty() {
            let mut manifest = hdcleaner_core::backups::Manifest::new("uninstall", &prog.name);
            manifest.entries = saved;
            let _ = manifest.save(&backup);
        }
        for r in &results {
            println!("  {:<12} {}", r.status, r.path);
        }
        if !pending.is_empty() {
            println!("{} items need administrator rights (one UAC prompt)...", pending.len());
            let ops: Vec<_> = pending
                .iter()
                .map(|(_, op)| hdcleaner_core::elevation::ElevatedOp::Removal { item: op.clone() })
                .collect();
            for r in hdcleaner_core::elevation::run_elevated_ops(&ops).map_err(explain)? {
                println!("  {:<12} {:?}", if r.ok { "removed" } else { "failed" }, r.message);
            }
        }
        if !dry_run {
            println!("registry backup: {}", backup.display());
        }
        Ok(())
    }
}

/// `hdcleaner uninstall --forced`: same steps as the GUI's forced dialog.
fn forced_uninstall(
    input: ForcedInput,
    program: Option<String>,
    unregistered: bool,
    end_processes: bool,
    run_official: bool,
    quiet: bool,
    leftovers: &LeftoverStep,
) -> Result<()> {
    let (all, _) = hdcleaner_core::programs::list_all();
    let t = hdcleaner_core::forced::resolve(&input, &all).map_err(explain)?;
    println!("target: {}", t.program.name);
    if let Some(f) = &t.program.install_location {
        println!("folder: {f}");
    }
    if let Some(v) = &t.version {
        let parts: Vec<&str> = [&v.product, &v.company, &v.version].into_iter().filter_map(|s| s.as_deref()).collect();
        if !parts.is_empty() {
            println!("file info: {}", parts.join(" · "));
        }
    }

    // Which registered program (if any) is the target; same rule as the GUI.
    if !t.matches.is_empty() {
        println!("\nregistered programs that may be the same product:");
        for m in &t.matches {
            println!("  {}  {}  ({})", m.program.name, m.program.version.as_deref().unwrap_or(""), m.reason);
        }
    }
    let chosen = if unregistered {
        None
    } else if let Some(n) = &program {
        let m = t
            .matches
            .iter()
            .find(|m| m.program.name.eq_ignore_ascii_case(n))
            .with_context(|| format!("\"{n}\" is not in the match list above"))?;
        Some(&m.program)
    } else {
        match t.matches.as_slice() {
            [m] if m.reason != "sameName" => Some(&m.program),
            [] => None,
            _ => {
                println!("  (not targeted: pass --program \"<name>\" to include its registration)");
                None
            }
        }
    };
    let prog = match chosen {
        Some(m) => hdcleaner_core::forced::target_from_match(m, &t.program),
        None => t.program.clone(),
    };
    println!("\ncleaning: {}{}", prog.name, if chosen.is_some() { " (registered)" } else { " (not registered)" });

    if !t.processes.is_empty() {
        println!("\nprocesses running from the folder:");
        for p in &t.processes {
            println!("  {:>6}  {}", p.pid, p.path.as_deref().unwrap_or(&p.name));
        }
        if end_processes {
            if !leftovers.confirm {
                bail!("nothing done: pass --confirm to end these processes");
            }
            for p in &t.processes {
                let Some(path) = &p.path else { continue };
                match hdcleaner_core::processes::terminate(p.pid, path) {
                    Ok(()) => println!("  ended {}", p.pid),
                    Err(e) => eprintln!("  could not end {}: {}", p.pid, explain(e)),
                }
            }
        } else {
            eprintln!("warning: files in use cannot be removed; pass --end-processes to end them (unsaved work is lost)");
        }
    }

    if run_official && chosen.is_some() && hdcleaner_core::uninstall::still_installed(&prog) {
        match hdcleaner_core::uninstall::command_for(&prog, quiet) {
            Ok(cmd) => {
                println!("\nofficial uninstaller: \"{}\" {}", cmd.file, cmd.params);
                if !leftovers.confirm {
                    bail!("nothing done: pass --confirm to run the uninstaller");
                }
                // Forced mode goes on whatever the uninstaller did.
                if let Err(e) = run_uninstaller(&prog, &cmd) {
                    eprintln!("uninstaller failed: {e:#}");
                }
            }
            Err(e) => println!("\nofficial uninstaller unavailable: {e}"),
        }
    }

    let others: Vec<_> = all.into_iter().filter(|p| Some(&p.id) != chosen.map(|c| &c.id)).collect();
    leftovers.run(&prog, &others)
}

/// `hdcleaner startup --disable|--enable|--remove NAME`.
fn startup_action(name: &str, action: &str, id: Option<&str>, confirm: bool, ours: &[String], db: Option<&hdcleaner_core::db::Database>) -> Result<()> {
    use hdcleaner_core::elevation::{run_elevated_ops, ElevatedOp};
    use hdcleaner_core::startup;
    let matches: Vec<_> = startup::list(ours).into_iter().filter(|i| id.map_or(i.name.eq_ignore_ascii_case(name), |id| i.id == id)).collect();
    let item = match matches.as_slice() {
        [one] => one.clone(),
        [] => bail!("no startup entry named \"{name}\""),
        many => {
            for i in many {
                eprintln!("  {}", i.id);
            }
            bail!("{} entries are named \"{name}\"; pick one with --id", many.len());
        }
    };
    println!("{}  [{}]  {}", item.name, if item.enabled { "enabled" } else { "disabled" }, item.command);
    if !confirm {
        bail!("nothing done: pass --confirm to {action} it");
    }
    let one = |op: ElevatedOp| -> Result<()> {
        println!("administrator rights needed (one UAC prompt)...");
        match run_elevated_ops(&[op]).map_err(explain)?.first() {
            Some(r) if r.ok => Ok(()),
            Some(r) => bail!("{}", r.message.clone().unwrap_or_default()),
            None => bail!("no result from the elevated helper"),
        }
    };
    if action == "remove" {
        let backup = hdcleaner_core::util::app_data_dir().join("backups").join(format!("cli-startup-{}", hdcleaner_core::util::now_unix_ms()));
        if let Some(op) = startup::remove(&item, &backup).map_err(explain)? {
            // Removal + approval cleanup in one helper run (one UAC prompt).
            let mut ops = vec![ElevatedOp::Removal { item: op }];
            ops.extend(startup::forget_approval_op(&item).map(|op| ElevatedOp::Startup { op }));
            println!("administrator rights needed (one UAC prompt)...");
            match run_elevated_ops(&ops).map_err(explain)?.first() {
                Some(r) if r.ok => {}
                Some(r) => bail!("{}", r.message.clone().unwrap_or_default()),
                None => bail!("no result from the elevated helper"),
            }
        }
        println!("removed; backup: {}", backup.display());
    } else {
        let on = action == "enable";
        if let Some(op) = startup::set_enabled(&item, on).map_err(explain)? {
            one(ElevatedOp::Startup { op })?;
        }
        if matches!(item.source, startup::Source::Service) {
            if let Some(db) = db {
                startup::remember_service(db, &item.key, !on);
            }
        }
        println!("{}", if on { "enabled" } else { "disabled" });
    }
    Ok(())
}

fn print_rows(tree: &ScanTree, ids: &[u32], json: bool) -> Result<()> {
    if json {
        let rows: Vec<_> = ids
            .iter()
            .map(|&i| {
                let n = tree.node(i);
                serde_json::json!({"path": tree.path(i), "size": n.size, "allocated": n.alloc,
                    "modified": filetime_to_unix_ms(n.modified)})
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for &i in ids {
            let n = tree.node(i);
            println!("{:>10}  {}  {}", bytes(n.size), datetime(filetime_to_unix_ms(n.modified)), tree.path(i));
        }
    }
    Ok(())
}
