use crate::model::{Config, Observation, Snapshot};
use anyhow::{ensure, Result};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub const WINDOW: usize = 21;
pub const MODEL_VERSION: &str = "margin-reversal-v1";

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Candidate {
    pub market: String,
    pub symbol: String,
    pub name: String,
    pub close: f64,
    pub balance_lots: u64,
    pub change_lots: i64,
    pub utilization_pct: Option<f64>,
    pub quota_basis: String,
    pub price_change_pct: f64,
    pub deleveraging_pct: Option<f64>,
    pub price_down_margin_up: bool,
    pub score: i32,
    pub breakdown: BTreeMap<String, i32>,
    pub stage: String,
}

#[derive(Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub model_version: String,
    pub date: NaiveDate,
    pub generated_at: chrono::DateTime<chrono::Utc>,
    pub data_observed_at: chrono::DateTime<chrono::Utc>,
    pub historical_import: bool,
    pub scope: String,
    pub quote_count: usize,
    pub margin_matched_count: usize,
    pub scored_count: usize,
    pub excluded: BTreeMap<String, String>,
    pub candidates: Vec<Candidate>,
    pub sources: BTreeMap<String, crate::model::Source>,
    pub market_maintenance_ratio: Option<f64>,
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}
fn minimum(values: &[f64]) -> f64 {
    values.iter().copied().fold(f64::INFINITY, f64::min)
}

pub fn score(obs: &[&Observation]) -> Option<Candidate> {
    if obs.len() < WINDOW {
        return None;
    }
    let obs = &obs[obs.len() - WINDOW..];
    let margins = obs
        .iter()
        .map(|o| o.margin.as_ref())
        .collect::<Option<Vec<_>>>()?;
    // Adjacent session financing balances must reconcile; corporate actions or gaps are excluded.
    if margins
        .windows(2)
        .any(|w| w[1].previous_lots != w[0].balance_lots)
    {
        return None;
    }
    let closes: Vec<_> = obs.iter().map(|o| o.quote.close).collect();
    if closes.iter().any(|p| !p.is_finite() || *p <= 0.0) {
        return None;
    }
    let last = obs[WINDOW - 1];
    let m = margins[WINDOW - 1];
    let peak = margins[..20].iter().map(|m| m.balance_lots).max()?;
    let deleveraging = (peak > 0).then(|| 100.0 * (m.balance_lots as f64 / peak as f64 - 1.0));
    let price_change = 100.0 * (closes[20] / closes[19] - 1.0);
    let divergence = price_change < 0.0 && m.change() > 0;
    let stable = minimum(&closes[16..]) >= minimum(&closes[..16]);
    let reversal = closes[20] > mean(&closes[16..])
        && mean(&closes[16..]) > mean(&closes[15..20])
        && closes[20] > closes[19];
    let prior_volume = obs[..20]
        .iter()
        .map(|o| o.quote.volume_shares as f64)
        .sum::<f64>()
        / 20.0;
    let utilization = m.utilization();
    let breakdown = BTreeMap::from([
        (
            "融資自前20日高點減少至少10%".into(),
            if deleveraging.is_some_and(|x| x <= -10.0) {
                30
            } else {
                0
            },
        ),
        (
            "近5日低點未跌破前16日低點".into(),
            if stable { 20 } else { 0 },
        ),
        (
            "收盤站上5日均線且均線與股價轉升".into(),
            if reversal { 20 } else { 0 },
        ),
        (
            "成交量達前20日均量1.2倍".into(),
            if prior_volume > 0.0 && last.quote.volume_shares as f64 >= prior_volume * 1.2 {
                10
            } else {
                0
            },
        ),
        (
            "融資使用率低於20%給10分、低於40%給5分".into(),
            match utilization {
                Some(x) if x < 20.0 => 10,
                Some(x) if x < 40.0 => 5,
                _ => 0,
            },
        ),
        (
            "收盤站上20日均線".into(),
            if closes[20] >= mean(&closes[1..]) {
                10
            } else {
                0
            },
        ),
        ("價跌融資增扣30分".into(), if divergence { -30 } else { 0 }),
        (
            "融資使用率達60%扣20分".into(),
            if utilization.is_some_and(|x| x >= 60.0) {
                -20
            } else {
                0
            },
        ),
    ]);
    let total: i32 = breakdown.values().sum();
    let risk = divergence || utilization.is_some_and(|x| x >= 60.0);
    Some(Candidate {
        market: last.quote.market.clone(),
        symbol: last.quote.symbol.clone(),
        name: last.quote.name.clone(),
        close: last.quote.close,
        balance_lots: m.balance_lots,
        change_lots: m.change(),
        utilization_pct: utilization,
        quota_basis: if last.quote.market == "TWSE" {
            "今日餘額 / 次一營業日限額（代理使用率）"
        } else {
            "資餘額 / 資限額"
        }
        .into(),
        price_change_pct: price_change,
        deleveraging_pct: deleveraging,
        price_down_margin_up: divergence,
        score: total.clamp(0, 100),
        breakdown,
        stage: if risk {
            "融資風險"
        } else if stable && reversal && deleveraging.is_some_and(|x| x <= -10.0) {
            "去槓桿後轉強"
        } else {
            "觀察"
        }
        .into(),
    })
}

pub fn expected_sessions(
    start: NaiveDate,
    end: NaiveDate,
    closed: &BTreeSet<NaiveDate>,
) -> Vec<NaiveDate> {
    let mut d = start;
    let mut out = vec![];
    while d <= end {
        if !matches!(d.weekday(), Weekday::Sat | Weekday::Sun) && !closed.contains(&d) {
            out.push(d);
        }
        d += Duration::days(1);
    }
    out
}

pub fn report(
    history: &[Snapshot],
    date: NaiveDate,
    config: &Config,
    closed: &BTreeSet<NaiveDate>,
) -> Result<Report> {
    let mut history: Vec<_> = history.iter().filter(|s| s.date <= date).collect();
    history.sort_by_key(|s| s.date);
    ensure!(
        history.windows(2).all(|w| w[0].date < w[1].date),
        "duplicate history date"
    );
    let current = history
        .last()
        .ok_or_else(|| anyhow::anyhow!("no history"))?;
    ensure!(current.date == date, "missing target session");
    ensure!(history.len() >= WINDOW, "warmup: need 21 complete sessions");
    let window = &history[history.len() - WINDOW..];
    let expected = expected_sessions(window[0].date, date, closed);
    ensure!(
        expected == window.iter().map(|s| s.date).collect::<Vec<_>>(),
        "missing trading session in lookback"
    );
    let mut candidates = vec![];
    let mut excluded = BTreeMap::new();
    for (k, obs) in &current.stocks {
        if !config.all_market && !config.watchlist.contains(&obs.quote.symbol) {
            continue;
        }
        let observations: Option<Vec<_>> = window.iter().map(|s| s.stocks.get(k)).collect();
        let candidate = observations.as_ref().and_then(|o| score(o));
        match candidate {
            Some(c) => candidates.push(c),
            None => {
                excluded.insert(
                    k.clone(),
                    "缺少連續21交易日行情/融資，或前後餘額不連續".into(),
                );
            }
        }
    }
    if !config.all_market {
        for symbol in &config.watchlist {
            ensure!(
                candidates.iter().any(|s| &s.symbol == symbol),
                "incomplete watchlist: {symbol}"
            );
        }
    }
    ensure!(
        candidates.len() >= 10,
        "fewer than 10 fully scored candidates"
    );
    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.market.cmp(&b.market))
            .then(a.symbol.cmp(&b.symbol))
    });
    let scored_count = candidates.len();
    candidates.truncate(10);
    Ok(Report {
        schema_version: 1,
        model_version: MODEL_VERSION.into(),
        date,
        generated_at: chrono::Utc::now(),
        data_observed_at: current.observed_at,
        historical_import: window.iter().any(|s| s.historical_import),
        scope: if config.all_market {
            "上市櫃：有可驗證融資及連續21交易日資料的普通股"
        } else {
            "固定十檔高價觀察名單，並非當日股價前十名"
        }
        .into(),
        quote_count: current.stocks.len(),
        margin_matched_count: current
            .stocks
            .values()
            .filter(|o| o.margin.is_some())
            .count(),
        scored_count,
        excluded,
        candidates,
        sources: current.sources.clone(),
        market_maintenance_ratio: None,
    })
}

#[derive(Serialize)]
pub struct Trade {
    pub symbol: String,
    pub market: String,
    pub signal_date: NaiveDate,
    pub entry_date: NaiveDate,
    pub exit_date: NaiveDate,
    pub net_return_pct: f64,
}
#[derive(Serialize)]
pub struct Backtest {
    pub model_version: String,
    pub assumption: String,
    pub trades: Vec<Trade>,
    pub avg_return_pct: Option<f64>,
    pub skipped_reports: usize,
}

pub fn backtest(
    history: &[Snapshot],
    config: &Config,
    closed: &BTreeSet<NaiveDate>,
    hold: usize,
) -> Result<Backtest> {
    ensure!(hold > 0, "hold must be positive");
    ensure!(
        history.windows(2).all(|w| w[0].date < w[1].date),
        "history must be sorted and unique"
    );
    let mut trades = vec![];
    let mut skipped_reports = 0;
    let mut next_available = BTreeMap::new();
    for i in WINDOW - 1..history.len() {
        let Ok(r) = report(&history[..=i], history[i].date, config, closed) else {
            skipped_reports += 1;
            continue;
        };
        for c in r
            .candidates
            .iter()
            .filter(|c| c.stage == "去槓桿後轉強" && c.score >= 60)
        {
            let k = crate::model::key(&c.market, &c.symbol);
            if next_available
                .get(&k)
                .is_some_and(|last: &usize| *last >= i)
            {
                continue;
            }
            // 00:00 next calendar day is conservative for retrospective imports; actual live
            // captures must exist before the entry session close at 13:30 Taiwan (05:30 UTC).
            let required_at = history[..=i]
                .iter()
                .rev()
                .take(WINDOW)
                .filter(|s| !s.historical_import)
                .map(|s| s.observed_at)
                .max();
            let Some(entry) = (i + 1..history.len()).find(|&j| {
                let close_at = history[j].date.and_hms_opt(5, 30, 0).unwrap().and_utc();
                required_at.is_none_or(|t| t < close_at)
            }) else {
                continue;
            };
            let exit = entry + hold;
            if exit >= history.len() {
                continue;
            }
            let sessions = expected_sessions(history[i].date, history[exit].date, closed);
            if sessions != history[i..=exit].iter().map(|s| s.date).collect::<Vec<_>>() {
                continue;
            }
            let (Some(buy), Some(sell)) =
                (history[entry].stocks.get(&k), history[exit].stocks.get(&k))
            else {
                continue;
            };
            // Cost model includes both fees, sell tax, and 10 bps slippage each side.
            let net = sell.quote.close * (1.0 - 0.001425 - 0.003 - 0.001)
                / (buy.quote.close * (1.0 + 0.001425 + 0.001))
                - 1.0;
            trades.push(Trade {
                symbol: c.symbol.clone(),
                market: c.market.clone(),
                signal_date: history[i].date,
                entry_date: history[entry].date,
                exit_date: history[exit].date,
                net_return_pct: net * 100.0,
            });
            next_available.insert(k, exit);
        }
    }
    let avg = (!trades.is_empty())
        .then(|| trades.iter().map(|t| t.net_return_pct).sum::<f64>() / trades.len() as f64);
    Ok(Backtest {model_version:MODEL_VERSION.into(),assumption:"研究用個股非重疊事件報酬，非投資組合績效。歷史補抓不知當時公布時間，假設最早次交易日收盤；即時快照另受實際取得時間限制。未還原除權息，未處理下市/存活者偏差；費用每邊0.1425%、賣稅0.3%、滑價每邊0.1%。不據此宣稱獲利能力。".into(),trades,avg_return_pct:avg,skipped_reports})
}
