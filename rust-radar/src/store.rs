use crate::{model::Snapshot, scoring::Report};
use anyhow::{ensure, Context, Result};
use chrono::NaiveDate;
use fs2::FileExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub fn lock(state: &Path) -> Result<File> {
    fs::create_dir_all(state)?;
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state.join("radar.lock"))?;
    f.try_lock_exclusive()
        .context("another radar process holds the lock")?;
    Ok(f) // OS releases lock on any exit, including SIGKILL.
}

pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}
pub fn json(path: &Path, data: &impl Serialize) -> Result<()> {
    atomic(path, &serde_json::to_vec_pretty(data)?)
}
pub fn snapshot_path(state: &Path, date: NaiveDate) -> PathBuf {
    state.join("history").join(format!("{date}.json"))
}

pub fn save_snapshot(state: &Path, snapshot: &Snapshot) -> Result<()> {
    let path = snapshot_path(state, snapshot.date);
    if path.exists() {
        let old: Snapshot = serde_json::from_slice(&fs::read(path)?)?;
        ensure!(
            old.stocks == snapshot.stocks,
            "official correction detected; immutable day retained for manual versioned review"
        );
        return Ok(());
    }
    json(&path, snapshot)
}
pub fn history(state: &Path) -> Result<Vec<Snapshot>> {
    read_history(state, None, None)
}

pub fn history_window(state: &Path, date: NaiveDate, limit: usize) -> Result<Vec<Snapshot>> {
    read_history(state, Some(date), Some(limit))
}

fn read_history(
    state: &Path,
    through: Option<NaiveDate>,
    limit: Option<usize>,
) -> Result<Vec<Snapshot>> {
    let dir = state.join("history");
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut paths = fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|p| p.extension().and_then(|x| x.to_str()) == Some("json"));
    paths.sort();
    if let Some(date) = through {
        paths.retain(|p| {
            p.file_stem()
                .and_then(|x| x.to_str())
                .is_some_and(|s| s <= date.to_string().as_str())
        });
    }
    if let Some(n) = limit {
        paths = paths.into_iter().rev().take(n).collect();
        paths.reverse();
    }
    let mut rows = vec![];
    for path in paths {
        if path.extension().and_then(|x| x.to_str()) == Some("json") {
            let s: Snapshot = serde_json::from_slice(&fs::read(&path)?)
                .with_context(|| format!("corrupt snapshot {}", path.display()))?;
            ensure!(
                path.file_stem().and_then(|x| x.to_str()) == Some(&s.date.to_string()),
                "snapshot path/date mismatch"
            );
            rows.push(s);
        }
    }
    rows.sort_by_key(|s| s.date);
    Ok(rows)
}

pub fn raw(state: &Path, date: NaiveDate, name: &str, body: &[u8]) -> Result<()> {
    let hash = format!("{:x}", Sha256::digest(body));
    let path = state
        .join("raw")
        .join(date.to_string())
        .join(format!("{name}-{hash}.json"));
    if !path.exists() {
        atomic(&path, body)?;
    }
    Ok(())
}

// The immutable daily report is also the recovery journal. A retry repairs latest.json
// after a crash between these writes. Monotonic dates prevent an old import rolling back live output.
pub fn publish(state: &Path, public: &Path, report: &Report) -> Result<bool> {
    let daily = state.join("reports").join(format!("{}.json", report.date));
    let newly_created = !daily.exists();
    if newly_created {
        json(&daily, report)?;
    }
    let saved = fs::read(&daily)?;
    let stored: serde_json::Value = serde_json::from_slice(&saved)?;
    ensure!(stored["model_version"] == report.model_version
        && stored["universe_mode"] == report.universe_mode
        && stored["fixed_watchlist"] == serde_json::to_value(&report.fixed_watchlist)?,
        "existing daily report uses a different watchlist/universe/model; manual versioned review required");
    if report.universe_mode == "fixed" {
        let symbols = stored["candidates"]
            .as_array()
            .context("invalid saved candidates")?
            .iter()
            .map(|c| c["symbol"].as_str())
            .collect::<Option<std::collections::BTreeSet<_>>>()
            .context("invalid saved symbol")?;
        let expected: std::collections::BTreeSet<_> =
            report.fixed_watchlist.iter().map(String::as_str).collect();
        ensure!(
            symbols == expected,
            "saved daily report contains incorrect fixed symbols"
        );
    }
    let latest = public.join("latest.json");
    if latest.exists() {
        let v: serde_json::Value = serde_json::from_slice(&fs::read(&latest)?)?;
        let date = v["date"].as_str().context("invalid published date")?;
        ensure!(
            date <= report.date.to_string().as_str(),
            "refuse to roll back latest report"
        );
        if date == report.date.to_string() {
            return Ok(false);
        }
    }
    atomic(&latest, &saved)?;
    Ok(newly_created)
}
