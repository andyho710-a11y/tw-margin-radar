use crate::model::{key, Margin, Observation, Quote, Snapshot, Source};
use anyhow::{anyhow, bail, ensure, Context, Result};
use chrono::{Datelike, NaiveDate, Utc};
use reqwest::blocking::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read, thread, time::Duration};

pub const SOURCES: [&str; 4] = ["twse_quotes", "twse_margin", "tpex_quotes", "tpex_margin"];

pub fn number(v: &Value) -> Result<f64> {
    let n = match v {
        Value::Number(n) => n.as_f64().context("invalid number")?,
        Value::String(s) => s
            .trim()
            .replace(',', "")
            .parse()
            .context("missing or invalid numeric field")?,
        _ => bail!("missing numeric field"),
    };
    ensure!(n.is_finite() && n >= 0.0, "nonfinite or negative number");
    Ok(n)
}
fn integer(v: &Value) -> Result<u64> {
    let n = number(v)?;
    ensure!(
        n.fract() == 0.0 && n <= 9_007_199_254_740_991.0,
        "invalid integer"
    );
    Ok(n as u64)
}
fn text(v: &Value) -> Result<&str> {
    v.as_str().context("missing text")
}

pub fn source_date(s: &str) -> Result<NaiveDate> {
    let compact = s.replace(['/', '-'], "");
    ensure!(
        compact.bytes().all(|b| b.is_ascii_digit()),
        "invalid source date characters"
    );
    let western = match compact.len() {
        7 => format!("{}{}", compact[..3].parse::<i32>()? + 1911, &compact[3..]),
        8 => compact,
        _ => bail!("invalid source date"),
    };
    Ok(NaiveDate::parse_from_str(&western, "%Y%m%d")?)
}

fn stock(s: &str) -> bool {
    s.len() == 4 && s.bytes().all(|x| x.is_ascii_digit()) && !s.starts_with('0')
}

fn table<'a>(v: &'a Value, field: &str) -> Result<&'a Value> {
    v["tables"]
        .as_array()
        .context("missing tables")?
        .iter()
        .find(|t| {
            t["fields"]
                .as_array()
                .is_some_and(|f| f.iter().any(|x| x == field))
        })
        .with_context(|| format!("schema changed: {field}"))
}
fn column(t: &Value, field: &str) -> Result<usize> {
    t["fields"]
        .as_array()
        .context("missing fields")?
        .iter()
        .position(|x| x == field)
        .with_context(|| format!("schema changed: {field}"))
}
fn rows(t: &Value) -> Result<&Vec<Value>> {
    t["data"].as_array().context("missing data")
}

pub fn parse_quotes(v: &Value, market: &str, date: NaiveDate) -> Result<BTreeMap<String, Quote>> {
    ensure!(
        source_date(text(&v["date"])?)? == date,
        "quote date mismatch"
    );
    let listed = market == "TWSE";
    let t = table(
        v,
        if listed {
            "證券代號"
        } else {
            "成交股數"
        },
    )?;
    let c = column(t, if listed { "證券代號" } else { "代號" })?;
    let n = column(t, if listed { "證券名稱" } else { "名稱" })?;
    let p = column(t, if listed { "收盤價" } else { "收盤" })?;
    let vol = column(t, "成交股數")?;
    let mut out = BTreeMap::new();
    for r in rows(t)? {
        let symbol = text(&r[c])?;
        if !stock(symbol) {
            continue;
        }
        // Only the documented no-trade marker is excluded. Other bad values fail closed.
        if ["--", "---", "----"].contains(&r[p].as_str().unwrap_or("").trim()) {
            continue;
        }
        let close = number(&r[p])?;
        ensure!(close > 0.0, "zero closing price");
        let q = Quote {
            market: market.into(),
            symbol: symbol.into(),
            name: text(&r[n])?.into(),
            close,
            volume_shares: integer(&r[vol])?,
        };
        ensure!(
            out.insert(key(market, symbol), q).is_none(),
            "duplicate quote"
        );
    }
    ensure!(!out.is_empty(), "no valid quotes");
    Ok(out)
}

pub fn parse_margin(v: &Value, market: &str, date: NaiveDate) -> Result<BTreeMap<String, Margin>> {
    ensure!(
        source_date(text(&v["date"])?)? == date,
        "margin date mismatch"
    );
    let t = table(
        v,
        if market == "TWSE" {
            "現金償還"
        } else {
            "前資餘額(張)"
        },
    )?;
    let indices: [usize; 7] = if market == "TWSE" {
        // TWSE repeats headers for financing/shorting: validate the financing prefix explicitly.
        let expected = [
            "代號",
            "名稱",
            "買進",
            "賣出",
            "現金償還",
            "前日餘額",
            "今日餘額",
            "次一營業日限額",
        ];
        for (i, label) in expected.iter().enumerate() {
            ensure!(t["fields"][i] == *label, "TWSE financing schema changed");
        }
        [0, 5, 6, 7, 2, 3, 4]
    } else {
        [
            "代號",
            "前資餘額(張)",
            "資餘額",
            "資限額",
            "資買",
            "資賣",
            "現償",
        ]
        .map(|s| column(t, s))
        .into_iter()
        .collect::<Result<Vec<_>>>()?
        .try_into()
        .unwrap()
    };
    let [code, prev, balance, quota, buys, sells, redemptions] = indices;
    let mut out = BTreeMap::new();
    for r in rows(t)? {
        let symbol = text(&r[code])?;
        if !stock(symbol) {
            continue;
        }
        let m = Margin {
            previous_lots: integer(&r[prev])?,
            balance_lots: integer(&r[balance])?,
            quota_lots: integer(&r[quota])?,
            buys_lots: integer(&r[buys])?,
            sells_lots: integer(&r[sells])?,
            redemptions_lots: integer(&r[redemptions])?,
        };
        ensure!(
            m.balance_lots as i128
                == m.previous_lots as i128 + m.buys_lots as i128
                    - m.sells_lots as i128
                    - m.redemptions_lots as i128,
            "financing balance equation failed: {symbol}"
        );
        ensure!(
            out.insert(key(market, symbol), m).is_none(),
            "duplicate margin"
        );
    }
    ensure!(!out.is_empty(), "no margin records");
    Ok(out)
}

pub fn urls(date: NaiveDate) -> BTreeMap<String, String> {
    let compact = date.format("%Y%m%d");
    let slash = date.format("%Y/%m/%d");
    BTreeMap::from([
        ("twse_quotes".into(), format!("https://www.twse.com.tw/exchangeReport/MI_INDEX?response=json&date={compact}&type=ALLBUT0999")),
        ("twse_margin".into(), format!("https://www.twse.com.tw/exchangeReport/MI_MARGN?response=json&date={compact}&selectType=ALL")),
        ("tpex_quotes".into(), format!("https://www.tpex.org.tw/www/zh-tw/afterTrading/dailyQuotes?date={slash}&id=&response=json")),
        ("tpex_margin".into(), format!("https://www.tpex.org.tw/www/zh-tw/margin/balance?date={slash}&id=&response=json")),
    ])
}

pub fn client() -> Result<Client> {
    Ok(Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("tw-margin-radar/0.1 public research")
        .build()?)
}
pub fn fetch(client: &Client, url: &str) -> Result<Vec<u8>> {
    let mut last = None;
    for attempt in 0..3 {
        let result = (|| -> Result<Vec<u8>> {
            let r = client.get(url).send()?.error_for_status()?;
            let mut data = Vec::new();
            r.take(20_000_001).read_to_end(&mut data)?;
            ensure!(data.len() <= 20_000_000, "response too large");
            serde_json::from_slice::<Value>(&data).context("not JSON")?;
            Ok(data)
        })();
        match result {
            Ok(data) => return Ok(data),
            Err(e) => last = Some(e),
        }
        if attempt < 2 {
            thread::sleep(Duration::from_secs(2_u64.pow(attempt + 1)));
        }
    }
    Err(anyhow!("API failed after 3 attempts: {}", last.unwrap()))
}

pub fn assemble(
    date: NaiveDate,
    bodies: &BTreeMap<String, Vec<u8>>,
    historical_import: bool,
) -> Result<Snapshot> {
    let mut stocks = BTreeMap::new();
    let mut sources = BTreeMap::new();
    let url_map = urls(date);
    let now = Utc::now();
    for (market, prefix) in [("TWSE", "twse"), ("TPEX", "tpex")] {
        let q: Value = serde_json::from_slice(
            bodies
                .get(&format!("{prefix}_quotes"))
                .context("missing quotes")?,
        )?;
        let m: Value = serde_json::from_slice(
            bodies
                .get(&format!("{prefix}_margin"))
                .context("missing margin")?,
        )?;
        let margins = parse_margin(&m, market, date)?;
        for (k, quote) in parse_quotes(&q, market, date)? {
            stocks.insert(
                k.clone(),
                Observation {
                    quote,
                    margin: margins.get(&k).cloned(),
                },
            );
        }
    }
    for name in SOURCES {
        sources.insert(
            name.into(),
            Source {
                url: url_map[name].clone(),
                sha256: format!("{:x}", Sha256::digest(&bodies[name])),
                fetched_at: now,
            },
        );
    }
    Ok(Snapshot {
        schema_version: 1,
        date,
        observed_at: now,
        historical_import,
        sources,
        stocks,
    })
}

// Official annual schedule contains both holidays and reopening milestones.
// Only explicit closed-market rows are interpreted as holidays.
pub fn holiday(v: &Value, date: NaiveDate) -> Result<bool> {
    ensure!(
        v["queryYear"].as_i64() == Some(date.year() as i64),
        "calendar year mismatch"
    );
    let mut closed = false;
    for row in v["data"].as_array().context("missing official calendar")? {
        let d = NaiveDate::parse_from_str(text(&row[0])?, "%Y-%m-%d")?;
        if d == date {
            let description = format!("{} {}", text(&row[1])?, text(&row[2])?);
            closed = ["休市", "放假", "補假", "無交易"]
                .iter()
                .any(|s| description.contains(s))
                && !description.contains("開始交易")
                && !description.contains("最後交易");
        }
    }
    Ok(closed)
}
