//! Minimal rule-based auto filing.
//!
//! Safety rules: only *moves* (never deletes), never overwrites (unique names),
//! skips partial downloads / hidden / system files / recently changed files,
//! every executed batch is journaled and can be undone.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub name: String,
    /// Lower-case, without dots. Empty = any.
    pub extensions: Vec<String>,
    /// Case-insensitive substrings of the file name. Empty = any.
    pub keywords: Vec<String>,
    /// Only files untouched for at least this many days. 0 = any age.
    pub idle_days: u32,
    pub target: String,
}

impl Rule {
    /// A rule must constrain *something*, otherwise it would file everything.
    pub fn is_meaningful(&self) -> bool {
        !self.target.trim().is_empty()
            && (!self.extensions.is_empty() || !self.keywords.is_empty() || self.idle_days > 0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SortCfg {
    pub enabled: bool,
    pub dry_run: bool,
    pub watch: Vec<String>,
    pub settle_seconds: u32,
    pub include_folders: bool,
    pub rules: Vec<Rule>,
}

impl Default for SortCfg {
    fn default() -> Self {
        SortCfg {
            enabled: false,
            dry_run: true,
            watch: Vec::new(),
            settle_seconds: 10,
            include_folders: false,
            rules: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub name: String,
    pub is_dir: bool,
    /// Last write time, Unix seconds.
    pub modified: u64,
    pub hidden_or_system: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Move {
    pub rule: String,
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Default, Debug)]
pub struct Report {
    pub moved: Vec<Move>,
    pub skipped: Vec<(PathBuf, String)>,
}

pub fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

const PARTIAL: &[&str] =
    &["crdownload", "part", "partial", "download", "tmp", "temp", "opdownload", "!ut", "aria2", "xltd", "td", "bc!"];

/// In-progress downloads, Office lock files and temp files are never touched.
pub fn is_transient(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.starts_with("~$") || lower.starts_with(".~") || lower == "desktop.ini" || lower == "thumbs.db" {
        return true;
    }
    match lower.rsplit_once('.') {
        Some((_, ext)) => PARTIAL.contains(&ext),
        None => false,
    }
}

fn ext_of(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_lowercase(),
        _ => String::new(),
    }
}

/// First matching rule for a candidate (all given criteria must hold).
pub fn rule_for<'a>(c: &Candidate, cfg: &'a SortCfg, now: u64, settle: u64) -> Option<&'a Rule> {
    if c.hidden_or_system || is_transient(&c.name) || (c.is_dir && !cfg.include_folders) {
        return None;
    }
    // Still being written / just arrived.
    if now.saturating_sub(c.modified) < settle {
        return None;
    }
    let ext = ext_of(&c.name);
    let lower = c.name.to_lowercase();
    cfg.rules.iter().filter(|r| r.is_meaningful()).find(|r| {
        (r.extensions.is_empty() || (!c.is_dir && r.extensions.contains(&ext)))
            && (r.keywords.is_empty() || r.keywords.iter().any(|k| lower.contains(&k.to_lowercase())))
            && (r.idle_days == 0 || now.saturating_sub(c.modified) >= r.idle_days as u64 * 86_400)
    })
}

/// `name`, or `name (2).ext`, `name (3).ext`… — the first that does not exist.
pub fn unique_dest(dir: &Path, name: &str, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let first = dir.join(name);
    if !exists(&first) {
        return Some(first);
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (name.to_owned(), String::new()),
    };
    (2..1000).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !exists(p))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}

/// Builds the move plan for every watched folder (reads directories only).
pub fn plan(cfg: &SortCfg, expand: impl Fn(&str) -> String) -> Vec<Move> {
    let now = now_unix();
    let settle = cfg.settle_seconds as u64;
    let mut moves = Vec::new();
    let mut claimed: Vec<PathBuf> = Vec::new();
    for w in &cfg.watch {
        let dir = PathBuf::from(expand(w));
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            let attrs = std::os::windows::fs::MetadataExt::file_attributes(&meta);
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(now);
            let c =
                Candidate { name: name.clone(), is_dir: meta.is_dir(), modified, hidden_or_system: attrs & 0x6 != 0 };
            let Some(rule) = rule_for(&c, cfg, now, settle) else { continue };
            let target = PathBuf::from(expand(&rule.target));
            if same_dir(&target, &dir) || target.starts_with(entry.path()) {
                continue;
            }
            let dest = unique_dest(&target, &name, |p| p.exists() || claimed.iter().any(|c| c == p));
            if let Some(to) = dest {
                claimed.push(to.clone());
                moves.push(Move { rule: rule.name.clone(), from: entry.path(), to });
            }
        }
    }
    moves
}

fn move_no_replace(from: &Path, to: &Path) -> Result<(), String> {
    use windows::Win32::Storage::FileSystem::{MOVEFILE_COPY_ALLOWED, MOVEFILE_WRITE_THROUGH, MoveFileExW};
    use windows::core::HSTRING;
    // No MOVEFILE_REPLACE_EXISTING: an existing destination makes this fail.
    unsafe {
        MoveFileExW(
            &HSTRING::from(from.as_os_str()),
            &HSTRING::from(to.as_os_str()),
            MOVEFILE_COPY_ALLOWED | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|e| e.message())
    }
}

/// Executes a plan and appends the successful moves to the undo journal.
pub fn execute(moves: &[Move], journal: &Path) -> Report {
    let mut report = Report::default();
    for m in moves {
        if !m.from.exists() {
            report.skipped.push((m.from.clone(), "源文件已不存在".into()));
            continue;
        }
        if m.to.exists() {
            report.skipped.push((m.from.clone(), "目标已存在".into()));
            continue;
        }
        if let Some(parent) = m.to.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            report.skipped.push((m.from.clone(), e.to_string()));
            continue;
        }
        match move_no_replace(&m.from, &m.to) {
            Ok(()) => report.moved.push(m.clone()),
            Err(e) => report.skipped.push((m.from.clone(), e)),
        }
    }
    if !report.moved.is_empty() {
        append_batch(journal, &report.moved);
    }
    report
}

// ---- undo journal: "#batch <unix> <n>" followed by "<from>\t<to>" lines ----

fn append_batch(journal: &Path, moved: &[Move]) {
    use std::io::Write;
    let mut text = format!("#batch {} {}\n", now_unix(), moved.len());
    for m in moved {
        text.push_str(&format!("{}\t{}\n", m.from.display(), m.to.display()));
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(journal);
    if let Ok(mut f) = file {
        let _ = f.write_all(text.as_bytes());
    }
}

/// Batches in the journal, oldest first: (unix time, moves).
pub fn read_journal(journal: &Path) -> Vec<(u64, Vec<(PathBuf, PathBuf)>)> {
    let Ok(text) = std::fs::read_to_string(journal) else { return Vec::new() };
    let mut out: Vec<(u64, Vec<(PathBuf, PathBuf)>)> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("#batch ") {
            let ts = rest.split_whitespace().next().and_then(|t| t.parse().ok()).unwrap_or(0);
            out.push((ts, Vec::new()));
        } else if let (Some((from, to)), Some(last)) = (line.split_once('\t'), out.last_mut()) {
            last.1.push((PathBuf::from(from), PathBuf::from(to)));
        }
    }
    out.retain(|(_, m)| !m.is_empty());
    out
}

fn write_journal(journal: &Path, batches: &[(u64, Vec<(PathBuf, PathBuf)>)]) {
    if batches.is_empty() {
        let _ = std::fs::remove_file(journal);
        return;
    }
    let mut text = String::new();
    for (ts, moves) in batches {
        text.push_str(&format!("#batch {ts} {}\n", moves.len()));
        for (f, t) in moves {
            text.push_str(&format!("{}\t{}\n", f.display(), t.display()));
        }
    }
    let _ = std::fs::write(journal, text);
}

/// Moves the most recent batch back. Files whose original name is taken again
/// come back under a unique name; nothing is ever overwritten.
pub fn undo_last(journal: &Path) -> Report {
    let mut batches = read_journal(journal);
    let mut report = Report::default();
    let Some((_, moves)) = batches.pop() else { return report };
    let mut remaining = Vec::new();
    for (from, to) in moves.into_iter().rev() {
        if !to.exists() {
            report.skipped.push((to, "归档后的文件已不存在".into()));
            continue;
        }
        let dir = from.parent().map(Path::to_path_buf).unwrap_or_default();
        let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(back) = unique_dest(&dir, &name, |p| p.exists()) else {
            report.skipped.push((to.clone(), "无法找到可用的原名".into()));
            remaining.push((from, to));
            continue;
        };
        let _ = std::fs::create_dir_all(&dir);
        match move_no_replace(&to, &back) {
            Ok(()) => report.moved.push(Move { rule: String::new(), from: to, to: back }),
            Err(e) => {
                report.skipped.push((to.clone(), e));
                remaining.push((from, to));
            }
        }
    }
    if !remaining.is_empty() {
        remaining.reverse();
        batches.push((now_unix(), remaining));
    }
    write_journal(journal, &batches);
    report
}

pub fn last_batch_len(journal: &Path) -> usize {
    read_journal(journal).last().map(|(_, m)| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(rules: Vec<Rule>) -> SortCfg {
        SortCfg { rules, ..SortCfg::default() }
    }

    fn rule(name: &str, ext: &[&str], kw: &[&str], idle: u32) -> Rule {
        Rule {
            name: name.into(),
            extensions: ext.iter().map(|s| s.to_string()).collect(),
            keywords: kw.iter().map(|s| s.to_string()).collect(),
            idle_days: idle,
            target: "X:\\t".into(),
        }
    }

    fn file(name: &str, age_days: u64) -> Candidate {
        Candidate {
            name: name.into(),
            is_dir: false,
            modified: 1_000_000_000 - age_days * 86_400,
            hidden_or_system: false,
        }
    }

    #[test]
    fn matching_rules() {
        let c = cfg(vec![rule("img", &["png"], &[], 0), rule("inv", &[], &["发票"], 0), rule("old", &["zip"], &[], 7)]);
        let now = 1_000_000_000;
        assert_eq!(rule_for(&file("a.PNG", 1), &c, now, 10).map(|r| r.name.as_str()), Some("img"));
        assert_eq!(rule_for(&file("3月发票.pdf", 1), &c, now, 10).map(|r| r.name.as_str()), Some("inv"));
        assert!(rule_for(&file("x.zip", 3), &c, now, 10).is_none());
        assert_eq!(rule_for(&file("x.zip", 8), &c, now, 10).map(|r| r.name.as_str()), Some("old"));
        // Partial downloads, hidden files, fresh files and folders are skipped.
        assert!(rule_for(&file("a.png.crdownload", 1), &c, now, 10).is_none());
        assert!(rule_for(&file("~$报告.png", 1), &c, now, 10).is_none());
        let mut hidden = file("a.png", 1);
        hidden.hidden_or_system = true;
        assert!(rule_for(&hidden, &c, now, 10).is_none());
        let fresh = Candidate { modified: now - 2, ..file("a.png", 0) };
        assert!(rule_for(&fresh, &c, now, 10).is_none());
        let dir = Candidate { is_dir: true, ..file("pics", 30) };
        assert!(rule_for(&dir, &c, now, 10).is_none());
        // A rule without criteria never matches.
        assert!(!rule("all", &[], &[], 0).is_meaningful());
    }

    #[test]
    fn unique_names_never_collide() {
        let taken = ["C:\\d\\a.txt", "C:\\d\\a (2).txt"];
        let exists = |p: &Path| taken.iter().any(|t| Path::new(t) == p);
        assert_eq!(unique_dest(Path::new("C:\\d"), "a.txt", exists).unwrap(), PathBuf::from("C:\\d\\a (3).txt"));
        assert_eq!(unique_dest(Path::new("C:\\d"), "b", exists).unwrap(), PathBuf::from("C:\\d\\b"));
    }

    #[test]
    fn execute_and_undo_roundtrip() {
        let base = std::env::temp_dir().join(format!("deskcrate-sort-test-{}", std::process::id()));
        let src = base.join("src");
        let dst = base.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.png"), b"new").unwrap();
        std::fs::write(dst.join("a.png"), b"existing").unwrap();
        let journal = base.join("undo.log");
        let mut c = cfg(vec![Rule { target: dst.to_string_lossy().into(), ..rule("img", &["png"], &[], 0) }]);
        c.watch = vec![src.to_string_lossy().into()];
        c.settle_seconds = 0;
        let moves = plan(&c, |s| s.to_owned());
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].to, dst.join("a (2).png"));
        let r = execute(&moves, &journal);
        assert_eq!(r.moved.len(), 1);
        // The pre-existing file was not overwritten.
        assert_eq!(std::fs::read(dst.join("a.png")).unwrap(), b"existing");
        assert_eq!(last_batch_len(&journal), 1);
        let u = undo_last(&journal);
        assert_eq!(u.moved.len(), 1);
        assert_eq!(std::fs::read(src.join("a.png")).unwrap(), b"new");
        assert_eq!(last_batch_len(&journal), 0);
        let _ = std::fs::remove_dir_all(&base);
    }
}
