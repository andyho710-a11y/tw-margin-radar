use chrono::{Duration, NaiveDate, TimeZone, Utc};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};
use tw_margin_radar::{
    model::{Config, Margin, Observation, Quote, Snapshot},
    scoring, sources, store,
};

fn date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 30).unwrap()
}
fn bodies() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        (
            "twse_quotes".into(),
            include_bytes!("fixtures/twse_quotes.json").to_vec(),
        ),
        (
            "twse_margin".into(),
            include_bytes!("fixtures/twse_margin.json").to_vec(),
        ),
        (
            "tpex_quotes".into(),
            include_bytes!("fixtures/tpex_quotes.json").to_vec(),
        ),
        (
            "tpex_margin".into(),
            include_bytes!("fixtures/tpex_margin.json").to_vec(),
        ),
    ])
}
fn series(count: usize) -> Vec<Snapshot> {
    let start = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
    let dates = scoring::expected_sessions(start, start + Duration::days(300), &BTreeSet::new());
    (0..count)
        .map(|i| {
            let stocks = Config::default()
                .watchlist
                .iter()
                .map(|symbol| {
                    let market = if ["5274", "6223"].contains(&symbol.as_str()) {
                        "TPEX"
                    } else {
                        "TWSE"
                    };
                    let balance = 1000 - i as u64 * 10;
                    let close = if i < 16 {
                        110.0 - i as f64
                    } else {
                        94.0 + (i - 15) as f64 * 2.0
                    };
                    let o = Observation {
                        quote: Quote {
                            market: market.into(),
                            symbol: symbol.clone(),
                            name: format!("stock{symbol}"),
                            close,
                            volume_shares: 1_000_000 + i as u64 * 50_000,
                        },
                        margin: Some(Margin {
                            previous_lots: balance + 10,
                            balance_lots: balance,
                            quota_lots: 10_000,
                            buys_lots: 0,
                            sells_lots: 10,
                            redemptions_lots: 0,
                        }),
                    };
                    (format!("{market}:{symbol}"), o)
                })
                .collect();
            Snapshot {
                schema_version: 1,
                date: dates[i],
                observed_at: Utc.from_utc_datetime(&dates[i].and_hms_opt(12, 0, 0).unwrap()),
                historical_import: false,
                sources: BTreeMap::new(),
                stocks,
            }
        })
        .collect()
}

#[test]
fn shipped_config_and_default_exactly_match_the_user_approved_ten() {
    // Deliberately independent of FIXED_WATCHLIST and Config::default(): this is
    // the user's acceptance oracle, not a comparison of two implementation copies.
    let expected = [
        "5274", "2059", "3443", "3653", "3008", "7769", "6515", "2383", "6223", "2454",
    ];
    let shipped: Config = serde_json::from_str(include_str!("../../config/radar.json")).unwrap();
    assert_eq!(shipped.watchlist, expected);
    assert_eq!(Config::default().watchlist, expected);
    assert_eq!(tw_margin_radar::model::FIXED_WATCHLIST, expected);
    assert_eq!(shipped.watchlist.iter().collect::<BTreeSet<_>>().len(), 10);
    shipped.validate().unwrap();
}

#[test]
fn wrong_missing_extra_duplicate_or_padded_symbols_fail_in_both_modes() {
    for all_market in [false, true] {
        let good = Config {
            all_market,
            ..Config::default()
        };
        let mut cases = vec![];
        let mut wrong = good.clone();
        wrong.watchlist[0] = "2330".into();
        cases.push(wrong);
        let mut missing = good.clone();
        missing.watchlist.pop();
        cases.push(missing);
        let mut extra = good.clone();
        extra.watchlist.push("2330".into());
        cases.push(extra);
        let mut duplicate = good.clone();
        duplicate.watchlist[0] = duplicate.watchlist[1].clone();
        cases.push(duplicate);
        let mut padded = good.clone();
        padded.watchlist[0] = " 5274".into();
        cases.push(padded);
        for invalid in cases {
            assert!(
                invalid.validate().is_err(),
                "bad config accepted: {:?}",
                invalid.watchlist
            );
        }
        good.validate().unwrap();
    }
}

#[test]
fn executable_rejects_wrong_ten_before_network_or_report_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let mut wrong = Config::default();
    wrong.watchlist[0] = "2330".into();
    let config = tmp.path().join("wrong.json");
    store::json(&config, &wrong).unwrap();
    let public = tmp.path().join("public");
    store::atomic(&public.join("latest.json"), b"last complete report").unwrap();
    let state = tmp.path().join("state");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_tw-margin-radar"))
        .arg("--config")
        .arg(config)
        .arg("--state")
        .arg(&state)
        .arg("--public")
        .arg(&public)
        .arg("update")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("fixed watchlist differs"));
    assert!(!state.join("raw").exists());
    assert!(!state.join("history").exists());
    assert_eq!(
        fs::read(public.join("latest.json")).unwrap(),
        b"last complete report"
    );
}

#[test]
fn all_market_scans_both_markets_outside_fixed_list_and_survives_missing_fixed_stock() {
    let mut h = series(21);
    for day in &mut h {
        for (market, symbol) in [("TWSE", "2330"), ("TPEX", "3529")] {
            let mut outside = day.stocks["TWSE:2059"].clone();
            outside.quote.market = market.into();
            outside.quote.symbol = symbol.into();
            day.stocks.insert(format!("{market}:{symbol}"), outside);
        }
        // Fixed-mode must fail, while the independent all-market universe still has 11 stocks.
        day.stocks.remove("TWSE:7769");
    }
    let fixed = Config::default();
    assert!(scoring::report(&h, h[20].date, &fixed, &BTreeSet::new()).is_err());
    let full = Config {
        all_market: true,
        ..fixed
    };
    let r = scoring::report(&h, h[20].date, &full, &BTreeSet::new()).unwrap();
    assert_eq!(r.scored_count, 11);
    assert_eq!(r.candidates.len(), 10);
    assert!(r
        .candidates
        .iter()
        .any(|c| c.market == "TWSE" && c.symbol == "2330"));
    assert!(r
        .candidates
        .iter()
        .any(|c| c.market == "TPEX" && c.symbol == "3529"));
}

#[test]
fn official_fixture_matches_both_markets_and_financing_not_shorting() {
    let s = sources::assemble(date(), &bodies(), true).unwrap();
    assert_eq!(s.stocks.len(), 10);
    assert_eq!(s.stocks["TWSE:2059"].quote.close, 9650.0);
    assert_eq!(s.stocks["TWSE:2059"].quote.volume_shares, 428_474);
    let m = s.stocks["TWSE:2059"].margin.as_ref().unwrap();
    assert_eq!(
        (m.previous_lots, m.balance_lots, m.quota_lots, m.change()),
        (671, 648, 23824, -23)
    );
    assert_eq!(
        s.stocks["TPEX:5274"].margin.as_ref().unwrap().balance_lots,
        151
    );
    assert_eq!(s.stocks["TPEX:5274"].quote.close, 15970.0);
    assert!(s.sources.values().all(|s| s.sha256.len() == 64));
}
#[test]
fn roc_dates_are_parsed_but_invalid_dates_cannot_panic() {
    assert_eq!(sources::source_date("115/10/30").unwrap(), date());
    for s in ["1151399", "漢字x", "xxxxxxx", "20261340"] {
        assert!(sources::source_date(s).is_err());
    }
}

#[test]
fn official_no_trade_marker_may_have_whitespace() {
    let mut v: Value = serde_json::from_slice(&bodies()["tpex_quotes"]).unwrap();
    v["tables"][0]["data"][0][2] = json!(" ---");
    let q = sources::parse_quotes(&v, "TPEX", date()).unwrap();
    assert!(!q.contains_key("TPEX:5274"));
    assert_eq!(q.len(), 1);
}
#[test]
fn mismatched_dates_fail_and_never_advance_output() {
    let mut b = bodies();
    let mut v: Value = serde_json::from_slice(&b["tpex_margin"]).unwrap();
    v["date"] = json!("20261029");
    b.insert("tpex_margin".into(), serde_json::to_vec(&v).unwrap());
    let dir = tempfile::tempdir().unwrap();
    store::atomic(&dir.path().join("latest.json"), b"previous complete report").unwrap();
    assert!(sources::assemble(date(), &b, false).is_err());
    assert_eq!(
        fs::read(dir.path().join("latest.json")).unwrap(),
        b"previous complete report"
    );
}
#[test]
fn financing_equation_and_schema_drift_are_rejected() {
    let mut v: Value = serde_json::from_slice(&bodies()["twse_margin"]).unwrap();
    v["tables"][0]["data"][0][6] = json!("100");
    assert!(sources::parse_margin(&v, "TWSE", date()).is_err());
    v["tables"][0]["fields"][2] = json!("new field");
    assert!(sources::parse_margin(&v, "TWSE", date()).is_err());
}
#[test]
fn missing_financing_is_not_zero_and_zero_quota_is_unknown() {
    let mut b = bodies();
    let mut v: Value = serde_json::from_slice(&b["twse_margin"]).unwrap();
    v["tables"][0]["data"].as_array_mut().unwrap().remove(0);
    b.insert("twse_margin".into(), serde_json::to_vec(&v).unwrap());
    let s = sources::assemble(date(), &b, true).unwrap();
    assert!(s.stocks["TWSE:2059"].margin.is_none());
    let m = Margin {
        previous_lots: 0,
        balance_lots: 0,
        quota_lots: 0,
        buys_lots: 0,
        sells_lots: 0,
        redemptions_lots: 0,
    };
    assert_eq!(m.utilization(), None);
}
#[test]
fn nonfinite_and_negative_numbers_fail() {
    for v in [
        json!("NaN"),
        json!("inf"),
        json!("--"),
        json!("-1"),
        Value::Null,
    ] {
        assert!(sources::number(&v).is_err());
    }
}
#[test]
fn daily_top_ten_have_auditable_score_and_risk_penalty() {
    let h = series(25);
    let r = scoring::report(&h, h[20].date, &Config::default(), &BTreeSet::new()).unwrap();
    assert_eq!(r.candidates.len(), 10);
    assert!(r.candidates.iter().all(|c| c.stage == "去槓桿後轉強"));
    for c in &r.candidates {
        assert_eq!(c.score, c.breakdown.values().sum::<i32>().clamp(0, 100));
    }
    let mut risk = h[..=20].to_vec();
    let o = risk[20].stocks.get_mut("TWSE:2059").unwrap();
    o.quote.close = 80.0;
    let m = o.margin.as_mut().unwrap();
    m.balance_lots = m.previous_lots + 20;
    m.buys_lots = 20;
    m.sells_lots = 0;
    let r = scoring::report(&risk, risk[20].date, &Config::default(), &BTreeSet::new()).unwrap();
    let c = r.candidates.iter().find(|c| c.symbol == "2059").unwrap();
    assert!(c.price_down_margin_up);
    assert_eq!(c.stage, "融資風險");
    assert!(c.score < 60);
}
#[test]
fn future_prices_cannot_change_past_scores() {
    let mut h = series(32);
    let before = scoring::report(&h, h[20].date, &Config::default(), &BTreeSet::new()).unwrap();
    for s in &mut h[21..] {
        for o in s.stocks.values_mut() {
            o.quote.close = 1e9;
            o.margin.as_mut().unwrap().balance_lots = 0;
        }
    }
    let after = scoring::report(&h, h[20].date, &Config::default(), &BTreeSet::new()).unwrap();
    assert_eq!(before.candidates, after.candidates);
}
#[test]
fn incomplete_history_or_watchlist_cannot_publish() {
    let mut h = series(25);
    assert!(scoring::report(&h[..20], h[19].date, &Config::default(), &BTreeSet::new()).is_err());
    h[20].stocks.remove("TWSE:2059");
    assert!(scoring::report(&h, h[20].date, &Config::default(), &BTreeSet::new()).is_err());
    h.remove(5);
    assert!(scoring::report(&h, h[20].date, &Config::default(), &BTreeSet::new()).is_err());
}
#[test]
fn corporate_action_or_gap_in_balances_is_excluded() {
    let mut h = series(21);
    h[10]
        .stocks
        .get_mut("TWSE:2059")
        .unwrap()
        .margin
        .as_mut()
        .unwrap()
        .previous_lots = 1;
    assert!(scoring::report(&h, h[20].date, &Config::default(), &BTreeSet::new()).is_err());
}
#[test]
fn holidays_and_reopening_milestones_are_distinct() {
    let v = json!({"queryYear":2026,"data":[["2026-01-01","元旦","放假1日"],["2026-01-02","開始交易日","開始交易"]]});
    assert!(sources::holiday(&v, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()).unwrap());
    assert!(!sources::holiday(&v, NaiveDate::from_ymd_opt(2026, 1, 2).unwrap()).unwrap());
    let mut closed = BTreeSet::new();
    closed.insert(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
    assert_eq!(
        scoring::expected_sessions(
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 1, 4).unwrap(),
            &closed
        )
        .len(),
        1
    );
}

#[test]
fn official_compensatory_holiday_and_settlement_only_day_are_closed() {
    let v = json!({"queryYear":2026,"data":[["2026-10-26","紀念日","於星期一補假。"],["2026-02-12","市場無交易，僅辦理結算交割作業",""]]});
    assert!(sources::holiday(&v, NaiveDate::from_ymd_opt(2026, 10, 26).unwrap()).unwrap());
    assert!(sources::holiday(&v, NaiveDate::from_ymd_opt(2026, 2, 12).unwrap()).unwrap());
}

#[test]
fn http_failure_retries_then_recovers_without_publishing() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/fixture", server.local_addr().unwrap());
    let worker = thread::spawn(move || {
        for attempt in 0..2 {
            let (mut stream, _) = server.accept().unwrap();
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer).unwrap();
            if attempt == 0 {
                stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            } else {
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    )
                    .unwrap();
            }
        }
    });
    assert_eq!(
        sources::fetch(&sources::client().unwrap(), &url).unwrap(),
        b"{}"
    );
    worker.join().unwrap();
}

#[test]
fn daily_window_avoids_loading_all_retained_history_or_future_dates() {
    let dir = tempfile::tempdir().unwrap();
    let h = series(35);
    for s in &h {
        store::save_snapshot(dir.path(), s).unwrap();
    }
    let window = store::history_window(dir.path(), h[25].date, scoring::WINDOW).unwrap();
    assert_eq!(window.len(), 21);
    assert_eq!(window[0].date, h[5].date);
    assert_eq!(window[20].date, h[25].date);
}
#[test]
fn os_lock_recovers_after_owner_exits() {
    let t = tempfile::tempdir().unwrap();
    let f = store::lock(t.path()).unwrap();
    assert!(store::lock(t.path()).is_err());
    drop(f);
    assert!(store::lock(t.path()).is_ok());
}
#[test]
fn corrupt_history_fails_instead_of_silently_skipping() {
    let t = tempfile::tempdir().unwrap();
    store::atomic(&t.path().join("history/2026-01-05.json"), b"{").unwrap();
    assert!(store::history(t.path()).is_err());
}
#[test]
fn report_publication_is_idempotent_and_recovers_interrupted_pointer() {
    let t = tempfile::tempdir().unwrap();
    let public = t.path().join("public");
    let h = series(22);
    let r = scoring::report(&h, h[20].date, &Config::default(), &BTreeSet::new()).unwrap();
    assert!(store::publish(t.path(), &public, &r).unwrap());
    let original = fs::read(public.join("latest.json")).unwrap();
    assert!(!store::publish(t.path(), &public, &r).unwrap());
    assert_eq!(original, fs::read(public.join("latest.json")).unwrap());
    fs::remove_file(public.join("latest.json")).unwrap();
    assert!(!store::publish(t.path(), &public, &r).unwrap());
    assert_eq!(original, fs::read(public.join("latest.json")).unwrap());
    let later = scoring::report(&h, h[21].date, &Config::default(), &BTreeSet::new()).unwrap();
    assert!(store::publish(t.path(), &public, &later).unwrap());
    assert!(store::publish(t.path(), &public, &r).is_err());
}

#[test]
fn legacy_or_wrong_watchlist_daily_report_cannot_silently_be_reused() {
    let tmp = tempfile::tempdir().unwrap();
    let h = series(21);
    let r = scoring::report(&h, h[20].date, &Config::default(), &BTreeSet::new()).unwrap();
    let daily = tmp.path().join("reports").join(format!("{}.json", r.date));
    let mut wrong = serde_json::to_value(&r).unwrap();
    wrong["fixed_watchlist"][0] = json!("2330");
    store::json(&daily, &wrong).unwrap();
    let public = tmp.path().join("public");
    store::atomic(&public.join("latest.json"), b"previous complete report").unwrap();
    assert!(store::publish(tmp.path(), &public, &r).is_err());
    assert_eq!(
        fs::read(public.join("latest.json")).unwrap(),
        b"previous complete report"
    );
    wrong.as_object_mut().unwrap().remove("fixed_watchlist");
    store::json(&daily, &wrong).unwrap();
    assert!(store::publish(tmp.path(), &public, &r).is_err());
}
#[test]
fn history_is_immutable_and_records_actual_import_time() {
    let t = tempfile::tempdir().unwrap();
    let mut s = sources::assemble(date(), &bodies(), true).unwrap();
    store::save_snapshot(t.path(), &s).unwrap();
    store::save_snapshot(t.path(), &s).unwrap();
    s.stocks.get_mut("TWSE:2059").unwrap().quote.close += 1.0;
    assert!(store::save_snapshot(t.path(), &s).is_err());
    let h = store::history(t.path()).unwrap();
    assert_eq!(h.len(), 1);
    assert!(h[0].historical_import);
}
#[test]
fn backtest_executes_after_signal_and_uses_costs() {
    let h = series(35);
    let b = scoring::backtest(&h, &Config::default(), &BTreeSet::new(), 5).unwrap();
    assert!(!b.trades.is_empty());
    for t in b.trades {
        assert!(t.entry_date > t.signal_date);
        assert!(t.exit_date > t.entry_date);
        let k = format!("{}:{}", t.market, t.symbol);
        let buy = h.iter().find(|s| s.date == t.entry_date).unwrap().stocks[&k]
            .quote
            .close;
        let sell = h.iter().find(|s| s.date == t.exit_date).unwrap().stocks[&k]
            .quote
            .close;
        assert!(t.net_return_pct < 100.0 * (sell / buy - 1.0));
    }
}
#[test]
fn late_data_cannot_be_traded_before_it_was_known() {
    let mut h = series(35);
    let late = h[25].date.and_hms_opt(12, 0, 0).unwrap().and_utc();
    for s in &mut h[..=20] {
        s.observed_at = late;
    }
    let b = scoring::backtest(&h, &Config::default(), &BTreeSet::new(), 5).unwrap();
    for t in b.trades.iter().filter(|t| t.signal_date == h[20].date) {
        assert!(t.entry_date > h[25].date);
    }
}
