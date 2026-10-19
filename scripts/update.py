"""TW market bottom radar. All market data is sourced, dated, and never fabricated."""
import datetime as dt
import json
import math
import os
import pathlib
import time
from statistics import mean
import requests

ROOT = pathlib.Path(__file__).resolve().parents[1]
DATA = ROOT / 'docs' / 'data'
CACHE = DATA / 'history_cache.json'
TWSE_QUOTES = 'https://openapi.twse.com.tw/v1/exchangeReport/STOCK_DAY_ALL'
TWSE_MARGIN = 'https://openapi.twse.com.tw/v1/exchangeReport/MI_MARGN'
HEADERS = {'User-Agent':'Mozilla/5.0 (compatible; TaiwanBottomRadar/2.0; public research)'}
WATCH = {'3529':'力旺','2356':'英業達','2328':'廣宇'}
OTC_WATCH = {'3529'}
TAIPEI = dt.timezone(dt.timedelta(hours=8))
MAX_FETCH = int(os.getenv('MAX_FETCH','75'))


def get_json(url, params=None):
    last = None
    for i in range(3):
        try:
            r = requests.get(url, params=params, headers=HEADERS, timeout=22)
            r.raise_for_status()
            return r.json()
        except (requests.RequestException, ValueError) as e:
            last = e
            if i < 2: time.sleep(1.5*(i+1))
    raise RuntimeError(f'Unable to fetch {url}: {last}')


def num(v):
    if isinstance(v,(int,float)): return float(v) if math.isfinite(v) else None
    if v is None: return None
    s = str(v).replace(',','').replace('+','').strip()
    if s in ('','--','---','N/A','除權息','-'): return None
    try:
        result = float(s)
        return result if math.isfinite(result) else None
    except ValueError: return None


def source_date(value):
    text = str(value or '').strip()
    if len(text) == 7 and text.isdigit():
        text = str(int(text[:3]) + 1911) + '-' + text[3:5] + '-' + text[5:]
    try: return dt.date.fromisoformat(text).isoformat()
    except ValueError: return None


def moving(vals, n):
    return mean(vals[-n:]) if len(vals)>=n else None


def score_stock(bars):
    """No lookahead: uses only supplied bars through current date."""
    if len(bars)<130: return None
    closes=[b['close'] for b in bars]
    vols=[b['volume'] for b in bars]
    if any(v<=0 for v in closes[-130:]): return None
    last=closes[-1]; lo=min(closes[-120:]); hi=max(closes[-120:])
    distance=(last/lo-1)*100
    peak_drop=(last/hi-1)*100
    ma20=moving(closes,20); ma60=moving(closes,60)
    ma20_prev=mean(closes[-25:-5]); vol_ratio=vols[-1]/max(1,mean(vols[-21:-1]))
    near_low=max(0,30-1.2*distance)  # 30
    base=10 if peak_drop<=-20 else (5 if peak_drop<=-10 else 0)
    stable=10 if min(closes[-15:])>=min(closes[-60:-15]) else 0
    turn=(8 if last>=ma20 else 0)+(7 if ma20>=ma20_prev else 0)
    # Financing and fundamental factors remain unavailable until trustworthy data is joined.
    # Explicitly score on observed price factors only; don't silently grant missing factors.
    observed=round(near_low+base+stable+turn,1)
    return {'score_observed':observed,'score_max':65,'distance_low_pct':round(distance,2),
            'drop_from_high_pct':round(peak_drop,2),'ma20':round(ma20,2),
            'ma60':round(ma60,2),'volume_ratio':round(vol_ratio,2),
            'stage':('底部轉強' if last>=ma20 and ma20>=ma20_prev and distance<=25 else
                     '底部觀察' if distance<=20 else '非底部區'),
            'close':last,'date':bars[-1]['date']}


def fetch_history(symbol, now=None):
    # Yahoo chart is an unofficial endpoint. Its availability is not guaranteed.
    code=symbol+('.TWO' if symbol in OTC_WATCH else '.TW')
    url=f'https://query1.finance.yahoo.com/v8/finance/chart/{code}'
    j=get_json(url, {'range':'2y','interval':'1d'})
    result=j['chart']['result'][0]
    times=result.get('timestamp',[])
    quote=result['indicators']['quote'][0]
    now = now or dt.datetime.now(TAIPEI)
    bars=[]
    for i,t in enumerate(times):
        c=quote['close'][i]; v=quote['volume'][i]
        if num(c) is None or num(v) is None or c<=0 or v<0: continue
        bar_date=dt.datetime.fromtimestamp(t,TAIPEI).date()
        # Use a conservative post-session cutoff, including for manual daytime runs.
        if bar_date>now.date() or (bar_date==now.date() and now.hour<16): continue
        bars.append({'date':bar_date.isoformat(),
                     'close':round(float(c),4),'volume':int(v)})
    return bars


def backtest(bars, hold=20):
    """Research-only, non-overlapping entry next close after signal; close-to-close proxy.
    Assumes buy fee 0.1425%*0.6, sell fee same, sell tax 0.3%; no slippage.
    Signals require 130 days history, trades never use future to generate signals.
    """
    fee=0.001425*0.6; tax=0.003; trades=[]; i=130
    while i+hold+1<len(bars):
        s=score_stock(bars[:i+1])
        if s and s['stage']=='底部轉強' and s['score_observed']>=42:
            entry=bars[i+1]['close']; exitp=bars[i+hold+1]['close']
            ret=((exitp*(1-fee-tax))/(entry*(1+fee))-1)*100
            trades.append({'signal_date':bars[i]['date'],'entry_date':bars[i+1]['date'],
                           'exit_date':bars[i+hold+1]['date'],'return_pct':round(ret,3)})
            i+=hold+1
        else: i+=1
    return {'hold_days':hold,'trades':len(trades),
            'win_rate_pct':round(100*sum(t['return_pct']>0 for t in trades)/len(trades),2) if trades else None,
            'avg_return_pct':round(mean(t['return_pct'] for t in trades),3) if trades else None,
            'note':'訊號收盤後，次日收盤價模擬進場；非實際可保證成交價。含假設費稅，不含滑價。'}


def load_cache():
    try: return json.loads(CACHE.read_text(encoding='utf-8'))
    except (OSError,ValueError): return {}


def main():
    DATA.mkdir(parents=True,exist_ok=True)
    current=dt.datetime.now(TAIPEI)
    today=current.date()
    now=current.isoformat(timespec='seconds')
    warnings=[]; quotes=[]; margin=[]; quotes_date=None
    try:
        raw=get_json(TWSE_QUOTES)
        for r in raw:
            symbol=str(r.get('Code','')).strip()
            close=num(r.get('ClosingPrice'))
            vol=num(r.get('TradeVolume'))
            if len(symbol)==4 and symbol.isdigit() and not symbol.startswith('0') and close and vol:
                quotes.append({'symbol':symbol,'name':r.get('Name',''), 'close':close,'volume':int(vol),
                               'date':source_date(r.get('Date'))})
        if not quotes: raise ValueError('No valid listed-stock quotes')
        # Endpoint may omit date. Do not misrepresent fetch time as quote date.
        quotes_date=next((r['date'] for r in quotes if r['date']),None)
    except Exception as e: warnings.append(f'上市行情取得失敗：{e}')
    try:
        raw=get_json(TWSE_MARGIN)
        margin=raw if isinstance(raw,list) else []
        if not margin: warnings.append('官方融資餘額端點沒有可用資料')
    except Exception as e: warnings.append(f'融資資料取得失敗：{e}')
    cache=load_cache(); symbols={q['symbol']:q for q in quotes}
    # Rotating scan covers all liquid listed stocks over multiple runs, not every stock every day.
    eligible=sorted((q for q in quotes if q['volume']>=200000),key=lambda x:x['symbol'])
    day=today.toordinal()
    if eligible:
        offset=(day*MAX_FETCH)%len(eligible)
        pool=(eligible[offset:]+eligible[:offset])[:MAX_FETCH]
    else: pool=[]
    selected=list(dict.fromkeys(list(WATCH)+[q['symbol'] for q in pool]))
    fetched=0; failed=0
    for symbol in selected:
        try:
            bars=fetch_history(symbol,current)
            if len(bars)<130: raise ValueError('fewer than 130 bars')
            cache[symbol]={'updated':now,'name':symbols.get(symbol,{}).get('name') or WATCH.get(symbol,'名稱待補'),'bars':bars}
            fetched+=1
        except Exception:
            failed+=1
        time.sleep(0.12)
    if failed: warnings.append(f'{failed} 檔歷史行情更新失敗，可能使用快取或略過')
    rows=[]
    for symbol,obj in cache.items():
        bars=obj.get('bars',[])
        if not bars: continue
        # Strict staleness: only show candidates with last bar within 7 calendar days.
        try: age=(today-dt.date.fromisoformat(bars[-1]['date'])).days
        except ValueError: continue
        if age<0 or age>7: continue
        score=score_stock(bars)
        if not score: continue
        quote=symbols.get(symbol,{})
        rows.append({'symbol':symbol,'name':quote.get('name') or obj.get('name') or WATCH.get(symbol,'名稱待補'),
                     'market':'上櫃自選' if symbol in OTC_WATCH else '上市',
                     'history_source':'Yahoo Finance chart (非官方)',
                     'last_history_date':bars[-1]['date'],**score,
                     'backtest':{str(h):backtest(bars,h) for h in (20,60,120)}})
    rows.sort(key=lambda r:(r['stage']=='底部轉強',r['score_observed']),reverse=True)
    if not rows:
        raise RuntimeError('沒有可評分的近期歷史行情；保留上次成功發布的資料。' + '；'.join(warnings))
    # Raw official margin data is retained separately, with schema transparency.
    (DATA/'official_margin_raw.json').write_text(json.dumps({'source':TWSE_MARGIN,'fetched_at':now,
        'records':margin},ensure_ascii=False),encoding='utf-8')
    CACHE.write_text(json.dumps(cache,ensure_ascii=False,separators=(',',':')),encoding='utf-8')
    out={'generated_at':now,'quotes_source':TWSE_QUOTES,'quotes_date':quotes_date,
         'official_margin_source':TWSE_MARGIN,'official_margin_records':len(margin),
         'market_maintenance_ratio':None,
         'market_maintenance_ratio_note':'未取得可驗證的官方全市場整戶維持率，故不顯示數值。',
         'coverage':{'listed_quotes':len(quotes),'liquid_universe':len(eligible),
                     'attempted_today':len(selected),'updated_today':fetched,
                     'cached_recent_scored':len(rows), 'scope':'上市股票輪替掃描＋上櫃自選力旺；非每日全市場。'},
         'warnings':warnings,'stocks':rows[:150]}
    (DATA/'radar.json').write_text(json.dumps(out,ensure_ascii=False,indent=2),encoding='utf-8')
    print(json.dumps({'updated':fetched,'scored':len(rows),'warnings':warnings},ensure_ascii=False))

if __name__=='__main__':main()
