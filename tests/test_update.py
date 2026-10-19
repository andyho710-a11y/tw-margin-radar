import datetime as dt
import json
import pytest
from scripts import update
from scripts.update import num,score_stock,backtest

def bars(n=220):
    return [{'date':f'2025-01-{i%28+1:02d}', 'close':float(35+0.015*i+((i%11)-5)*.08),'volume':250000+i*100} for i in range(n)]

def test_num():
    assert num('1,234')==1234
    assert num('--') is None
    assert num(None) is None

def test_score_short():
    assert score_stock(bars(80)) is None

def test_score_bounds():
    s=score_stock(bars())
    assert 0<=s['score_observed']<=65
    assert s['score_max']==65

def test_backtest_no_lookahead():
    result=backtest(bars(),20)
    assert result['hold_days']==20
    assert result['trades']>=0

@pytest.mark.parametrize('value', ['nan', 'inf', '-inf', float('nan')])
def test_nonfinite_values_are_missing(value):
    assert num(value) is None

def test_official_roc_date():
    assert update.source_date('1151016') == '2026-10-16'
    assert update.source_date(None) is None
    assert update.source_date('1151399') is None

def test_otc_history_and_completed_session(monkeypatch):
    now = dt.datetime(2026, 10, 19, 15, tzinfo=update.TAIPEI)
    timestamps = [int(dt.datetime(2026, 10, day, 9, tzinfo=update.TAIPEI).timestamp()) for day in (16, 19)]
    requested = []
    def fake_get(url, params):
        requested.append(url)
        return {'chart': {'result': [{'timestamp': timestamps, 'indicators': {'quote': [{'close': [100, 110], 'volume': [1000, 2000]}]}}]}}
    monkeypatch.setattr(update, 'get_json', fake_get)
    assert [b['date'] for b in update.fetch_history('3529', now)] == ['2026-10-16']
    assert requested[0].endswith('3529.TWO')
    assert len(update.fetch_history('2356', now.replace(hour=18))) == 2
    assert requested[-1].endswith('2356.TW')

def test_failed_update_preserves_last_good_data(monkeypatch, tmp_path):
    data = tmp_path / 'data'
    data.mkdir()
    radar = data / 'radar.json'
    radar.write_text('{"previous":true}', encoding='utf-8')
    monkeypatch.setattr(update, 'DATA', data)
    monkeypatch.setattr(update, 'CACHE', data / 'history_cache.json')
    monkeypatch.setattr(update, 'get_json', lambda *a, **k: [])
    monkeypatch.setattr(update, 'fetch_history', lambda *a: [])
    monkeypatch.setattr(update.time, 'sleep', lambda _: None)
    with pytest.raises(RuntimeError, match='保留上次'):
        update.main()
    assert json.loads(radar.read_text(encoding='utf-8')) == {'previous': True}

def test_backtest_signal_only_sees_past_bars(monkeypatch):
    history = [{'date': (dt.date(2025, 1, 1) + dt.timedelta(days=i)).isoformat(), 'close': 100 + i, 'volume': 1000} for i in range(170)]
    seen = []
    def signal(prefix):
        seen.append(len(prefix))
        return {'stage': '底部轉強', 'score_observed': 50}
    monkeypatch.setattr(update, 'score_stock', signal)
    result = update.backtest(history, hold=20)
    assert seen == [131]
    expected = ((251 * (1 - 0.000855 - 0.003)) / (231 * (1 + 0.000855)) - 1) * 100
    assert result['trades'] == 1
    assert result['avg_return_pct'] == round(expected, 3)
