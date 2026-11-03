use anyhow::{bail, ensure, Context, Result};
use chrono::{Datelike, Duration, FixedOffset, NaiveDate, Timelike, Utc, Weekday};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::Duration as Wait,
};
use tw_margin_radar::{
    model::{Config, Snapshot},
    scoring, sources, store,
};

#[derive(Parser)]
#[command(
    version,
    about = "Official TWSE/TPEX margin radar; local files only, no trading or remote publishing"
)]
struct Cli {
    #[arg(long, default_value = "var/radar", global = true)]
    state: PathBuf,
    #[arg(long, default_value = "docs/rust/data", global = true)]
    public: PathBuf,
    #[arg(long, default_value = "config/radar.json", global = true)]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Stage today's official prices after 16:00 Taiwan; never publish a report.
    Prices,
    /// Collect today's aligned prices/margin after 18:00 Taiwan; publish only a complete report.
    Update,
    /// Import official historical sessions. Does not publish; records actual import time.
    Import {
        #[arg(long)]
        from: NaiveDate,
        #[arg(long)]
        to: NaiveDate,
    },
    /// Render an offline review report. Explicit date, separate output, no deployment.
    Preview {
        #[arg(long)]
        date: NaiveDate,
        #[arg(long, default_value = "preview/data/latest.json")]
        output: PathBuf,
    },
    /// Historical, individual-event research. Never executes trades.
    Backtest {
        #[arg(long, default_value_t = 5)]
        hold: usize,
        #[arg(long, default_value = "var/backtest.json")]
        output: PathBuf,
    },
    /// Check whether the last complete report matches today's required session.
    Health,
}

fn taipei_now() -> chrono::DateTime<FixedOffset> {
    Utc::now().with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap())
}

fn config(path: &Path) -> Result<Config> {
    let c: Config = serde_json::from_slice(&fs::read(path).context("config must exist")?)?;
    c.validate()?;
    Ok(c)
}

fn calendar(state: &Path, year: i32, client: &reqwest::blocking::Client) -> Result<Value> {
    let url = format!(
        "https://www.twse.com.tw/holidaySchedule/holidaySchedule?response=json&queryYear={}",
        year - 1911
    );
    let bytes = sources::fetch(client, &url)?;
    let v: Value = serde_json::from_slice(&bytes)?;
    // Fail if schema or year changed before saving a usable calendar.
    sources::holiday(
        &v,
        NaiveDate::from_ymd_opt(year, 1, 1).context("calendar year")?,
    )?;
    store::atomic(&state.join("calendar").join(format!("{year}.json")), &bytes)?;
    Ok(v)
}
fn closed_dates(state: &Path, config: &Config) -> Result<BTreeSet<NaiveDate>> {
    let mut out: BTreeSet<_> = config.closed_dates.iter().copied().collect();
    let dir = state.join("calendar");
    if !dir.exists() {
        bail!("missing official calendar");
    }
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let v: Value = serde_json::from_slice(&fs::read(p)?)?;
        for r in v["data"].as_array().context("calendar data")? {
            let d = NaiveDate::parse_from_str(r[0].as_str().context("calendar date")?, "%Y-%m-%d")?;
            if sources::holiday(&v, d)? {
                out.insert(d);
            }
        }
    }
    Ok(out)
}
fn is_closed(d: NaiveDate, closed: &BTreeSet<NaiveDate>) -> bool {
    matches!(d.weekday(), Weekday::Sat | Weekday::Sun) || closed.contains(&d)
}

fn collect(
    state: &Path,
    date: NaiveDate,
    historical: bool,
    client: &reqwest::blocking::Client,
) -> Result<Snapshot> {
    let mut bodies = BTreeMap::new();
    for (name, url) in sources::urls(date) {
        let body = sources::fetch(client, &url).with_context(|| format!("fetch {name}"))?;
        store::raw(state, date, &name, &body)?;
        bodies.insert(name, body);
        thread::sleep(Wait::from_millis(400)); // bounded and polite; never parallel per-stock fanout
    }
    let snapshot = sources::assemble(date, &bodies, historical)?;
    store::save_snapshot(state, &snapshot)?;
    Ok(snapshot)
}

fn status(state: &Path, phase: &str, date: Option<NaiveDate>, message: &str) -> Result<()> {
    let event = json!({"at":Utc::now(),"phase":phase,"date":date,"message":message});
    store::json(&state.join("status.json"), &event)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join("events.jsonl"))?;
    writeln!(file, "{event}")?;
    println!("{event}");
    Ok(())
}

fn run(cli: &Cli) -> Result<()> {
    let _lock = store::lock(&cli.state)?;
    let config = config(&cli.config)?;
    // Preserve host headroom. Never consume the trading VM's last free space.
    if matches!(
        cli.command,
        Command::Prices | Command::Update | Command::Import { .. }
    ) {
        ensure!(
            fs2::available_space(&cli.state)? >= 1_073_741_824,
            "less than 1 GiB free disk; collection refused"
        );
    }
    let now = taipei_now();
    let today = now.date_naive();
    match &cli.command {
        Command::Import { from, to } => {
            ensure!(
                from <= to && *to < today,
                "import must be past dates, ordered"
            );
            ensure!(
                (*to - *from).num_days() <= 370,
                "import at most 370 calendar days per invocation"
            );
            let client = sources::client()?;
            for year in from.year()..=to.year() {
                calendar(&cli.state, year, &client)?;
            }
            let closed = closed_dates(&cli.state, &config)?;
            for date in scoring::expected_sessions(*from, *to, &closed) {
                if !store::snapshot_path(&cli.state, date).exists() {
                    collect(&cli.state, date, true, &client)?;
                }
                status(
                    &cli.state,
                    "imported",
                    Some(date),
                    "historical import; publication time unknown; no report published",
                )?;
                thread::sleep(Wait::from_secs(2));
            }
        }
        Command::Preview { date, output } => {
            ensure!(*date <= today, "future date");
            let closed = closed_dates(&cli.state, &config)?;
            let r = scoring::report(
                &store::history_window(&cli.state, *date, scoring::WINDOW)?,
                *date,
                &config,
                &closed,
            )?;
            store::json(output, &r)?;
            status(
                &cli.state,
                "preview",
                Some(*date),
                "offline review output; not a production publication",
            )?;
        }
        Command::Backtest { hold, output } => {
            let closed = closed_dates(&cli.state, &config)?;
            store::json(
                output,
                &scoring::backtest(&store::history(&cli.state)?, &config, &closed, *hold)?,
            )?;
            status(
                &cli.state,
                "backtest",
                None,
                "research event returns written",
            )?;
        }
        Command::Prices | Command::Update | Command::Health => {
            let cutoff = if matches!(cli.command, Command::Prices) {
                16
            } else {
                18
            };
            if !matches!(cli.command, Command::Health) && now.hour() < cutoff {
                return status(
                    &cli.state,
                    "outside_window",
                    Some(today),
                    "Taiwan post-close cutoff not reached",
                );
            }
            // Weekend skips need no API. Every weekday obtains a current official calendar.
            if matches!(today.weekday(), Weekday::Sat | Weekday::Sun) {
                return status(&cli.state, "market_closed", Some(today), "weekend");
            }
            let client = sources::client()?;
            calendar(&cli.state, today.year(), &client)?;
            let closed = closed_dates(&cli.state, &config)?;
            if is_closed(today, &closed) {
                return status(
                    &cli.state,
                    "market_closed",
                    Some(today),
                    "official holiday/explicit emergency closure",
                );
            }
            if matches!(cli.command, Command::Health) {
                let v: Value = serde_json::from_slice(&fs::read(cli.public.join("latest.json"))?)?;
                let d = NaiveDate::parse_from_str(
                    v["date"].as_str().context("report date")?,
                    "%Y-%m-%d",
                )?;
                // Before evening deadline, previous complete session is acceptable.
                let mut required = today;
                if now.hour() < 23 {
                    required -= Duration::days(1);
                }
                while is_closed(required, &closed) {
                    required -= Duration::days(1);
                }
                ensure!(d >= required && d <= today, "stale or future report");
                return status(
                    &cli.state,
                    "healthy",
                    Some(d),
                    "complete report within session deadline",
                );
            }
            if matches!(cli.command, Command::Prices) {
                for (name, url) in sources::urls(today)
                    .into_iter()
                    .filter(|(name, _)| name.ends_with("quotes"))
                {
                    let bytes = sources::fetch(&client, &url)?;
                    let v = serde_json::from_slice(&bytes)?;
                    sources::parse_quotes(
                        &v,
                        if name.starts_with("twse") {
                            "TWSE"
                        } else {
                            "TPEX"
                        },
                        today,
                    )?;
                    store::raw(&cli.state, today, &name, &bytes)?;
                }
                return status(
                    &cli.state,
                    "prices_staged",
                    Some(today),
                    "official prices only; no partial report published",
                );
            }
            if !store::snapshot_path(&cli.state, today).exists() {
                collect(&cli.state, today, false, &client)?;
            }
            let r = scoring::report(
                &store::history_window(&cli.state, today, scoring::WINDOW)?,
                today,
                &config,
                &closed,
            )?;
            let fresh = store::publish(&cli.state, &cli.public, &r)?;
            status(
                &cli.state,
                if fresh {
                    "published"
                } else {
                    "already_published"
                },
                Some(today),
                "complete aligned report; atomic output",
            )?;
        }
    }
    Ok(())
}
fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        // Fixed error prefix; source bodies, local credentials and webhook URLs are never logged.
        let message = format!("{e:#}");
        eprintln!(
            "{}",
            json!({"at":Utc::now(),"phase":"failed","message":message})
        );
        // Lock contention must not mutate the running job's status/event files.
        if let Ok(_lock) = store::lock(&cli.state) {
            let _ = status(&cli.state, "failed", None, &message);
        }
        std::process::exit(1);
    }
}
