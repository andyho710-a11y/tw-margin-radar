use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// User-approved fixed universe. Updating it requires an explicit reviewed change.
pub const FIXED_WATCHLIST: [&str; 10] = [
    "5274", "2059", "3443", "3653", "3008", "7769", "6515", "2383", "6223", "2454",
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Quote {
    pub market: String,
    pub symbol: String,
    pub name: String,
    pub close: f64,
    pub volume_shares: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Margin {
    pub previous_lots: u64,
    pub balance_lots: u64,
    pub quota_lots: u64,
    pub buys_lots: u64,
    pub sells_lots: u64,
    pub redemptions_lots: u64,
}

impl Margin {
    pub fn change(&self) -> i64 {
        self.balance_lots as i64 - self.previous_lots as i64
    }
    pub fn utilization(&self) -> Option<f64> {
        (self.quota_lots > 0).then(|| 100.0 * self.balance_lots as f64 / self.quota_lots as f64)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Observation {
    pub quote: Quote,
    // None is ineligible/unavailable, never zero financing.
    pub margin: Option<Margin>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Source {
    pub url: String,
    pub sha256: String,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub date: NaiveDate,
    pub observed_at: DateTime<Utc>,
    pub historical_import: bool,
    pub sources: BTreeMap<String, Source>,
    pub stocks: BTreeMap<String, Observation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub watchlist: Vec<String>,
    pub all_market: bool,
    pub closed_dates: Vec<NaiveDate>,
}

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        use anyhow::ensure;
        use std::collections::BTreeSet;
        ensure!(
            self.watchlist.len() == FIXED_WATCHLIST.len(),
            "fixed watchlist must contain exactly ten approved symbols"
        );
        let actual: BTreeSet<_> = self.watchlist.iter().map(String::as_str).collect();
        ensure!(
            actual.len() == self.watchlist.len(),
            "duplicate fixed watchlist symbol"
        );
        let expected: BTreeSet<_> = FIXED_WATCHLIST.into_iter().collect();
        ensure!(
            actual == expected,
            "fixed watchlist differs from user-approved symbols: missing={:?}, unexpected={:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        );
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            watchlist: FIXED_WATCHLIST.map(String::from).to_vec(),
            all_market: false,
            closed_dates: vec![],
        }
    }
}

pub fn key(market: &str, symbol: &str) -> String {
    format!("{market}:{symbol}")
}
