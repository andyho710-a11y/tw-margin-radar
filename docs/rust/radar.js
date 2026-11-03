"use strict";
const $=id=>document.getElementById(id);
const escapeHtml=s=>String(s??"").replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
const n=(x,d=0)=>Number.isFinite(x)?x.toLocaleString("zh-TW",{maximumFractionDigits:d}):"未提供";
const fixedWatchlist=["5274","2059","3443","3653","3008","7769","6515","2383","6223","2454"];
const exactSymbols=values=>Array.isArray(values)&&values.length===10&&new Set(values).size===10&&fixedWatchlist.every(s=>values.includes(s));
let stocks=[];
function render(){
  const q=$("search").value.trim(),stage=$("stage").value;
  const rows=stocks.filter(s=>(!q||(s.symbol+s.name).includes(q))&&(!stage||s.stage===stage));
  $("empty").hidden=rows.length>0;
  $("stocks").innerHTML=rows.map(s=>`<article class="stock"><div class="stock-head"><div><span class="tag ${s.stage==="融資風險"?"risk":""}">${escapeHtml(s.stage)}</span><h3>${escapeHtml(s.symbol)} ${escapeHtml(s.name)}</h3><small class="muted">${escapeHtml(s.market)}</small></div><div class="score">${n(s.score)}<small> / 100</small></div></div><div class="metrics"><div><span>收盤 / 漲跌</span><b>${n(s.close,2)} / ${n(s.price_change_pct,2)}%</b></div><div><span>融資餘額 / 日增減（張）</span><b>${n(s.balance_lots)} / ${s.change_lots>0?"+":""}${n(s.change_lots)}</b></div><div><span>融資使用率</span><b>${n(s.utilization_pct,2)}${s.utilization_pct===null?"":"%"}</b></div><div><span>較前20日融資高點</span><b>${n(s.deleveraging_pct,2)}${s.deleveraging_pct===null?"":"%"}</b></div></div><details><summary>查看評分依據與使用率定義</summary><dl>${Object.entries(s.breakdown).map(([k,v])=>`<div><dt>${escapeHtml(k)}</dt><dd>${v>0?"+":""}${n(v)}</dd></div>`).join("")}</dl><p class="muted">${escapeHtml(s.quota_basis)}</p></details></article>`).join("");
}
async function load(){
  $("refresh").disabled=true;
  try{
    const response=await fetch("data/latest.json",{cache:"no-store"});
    if(!response.ok)throw Error(`HTTP ${response.status}`);
    const d=await response.json();
    if(d.schema_version!==1||!/^\d{4}-\d{2}-\d{2}$/.test(d.date)||!Array.isArray(d.candidates)||d.candidates.length!==10)throw Error("完整報告格式不符");
    if(!d.candidates.every(s=>s.breakdown&&Number.isInteger(s.score)&&s.score>=0&&s.score<=100))throw Error("候選評分格式不符");
    if(!["fixed","all_market"].includes(d.universe_mode)||!exactSymbols(d.fixed_watchlist))throw Error("固定監控名單與指定十檔不符");
    if(new Set(d.candidates.map(s=>s.market+":"+s.symbol)).size!==10)throw Error("候選股票重複");
    if(d.universe_mode==="fixed"&&!exactSymbols(d.candidates.map(s=>s.symbol)))throw Error("固定模式候選含錯誤或遺漏股票");
    const generated=Date.parse(d.generated_at), observed=Date.parse(d.data_observed_at), session=Date.parse(d.date+"T00:00:00+08:00");
    if(!Number.isFinite(session)||new Date(session+8*3600000).toISOString().slice(0,10)!==d.date||!Number.isFinite(generated)||!Number.isFinite(observed)||generated>Date.now()+300000||observed>generated||session>Date.now())throw Error("資料時間無效或來自未來");
    stocks=d.candidates;
    $("date").textContent=d.date;
    $("coverage").textContent=`${n(d.scored_count)} / ${n(d.quote_count)}`;
    $("scope").textContent=d.scope+"。"+(d.universe_mode==="fixed"?"固定監控："+fixedWatchlist.join("、"):"全市場獨立掃描，不限於固定十檔。")+"。高排名不代表符合買進條件。";
    const age=(Date.now()-(Date.parse(d.date)+16*3600000))/86400000;
    const stale=age>2;
    $("status").className="notice"+((stale||d.historical_import)?" warn":"");
    $("status").textContent=(stale?"交易日已超過兩天，可能休市或更新失敗，請核對來源日期。 ":"已載入最新成功產生的完整報告。 ")+(d.historical_import?"包含歷史補抓；不能據此證明當時可取得資料。":"")+"行情與融資已對齊同一交易日。";
    $("generated").textContent=`產生時間：${d.generated_at}；本日資料取得時間：${d.data_observed_at}；模型：${d.model_version}`;
    $("sources").replaceChildren();
    for(const [name,source]of Object.entries(d.sources||{})){
      const url=new URL(source.url);
      if(url.protocol!=="https:"||!["www.twse.com.tw","www.tpex.org.tw"].includes(url.hostname))continue;
      const p=document.createElement("p"),a=document.createElement("a");a.href=url.href;a.textContent=name;a.rel="noopener";p.append(a,document.createTextNode(` · SHA256 ${source.sha256}`));$("sources").append(p);
    }
    render();
  }catch(e){stocks=[];render();$("date").textContent="—";$("coverage").textContent="—";$("sources").replaceChildren();$("generated").textContent="";$("status").className="notice warn";$("status").textContent=`尚無可用的完整 Rust 報告（${e.message}）。首次需要累積或補抓連續 21 個交易日；資料未齊時不顯示部分排名。`;}
  finally{$("refresh").disabled=false;}
}
$("search").addEventListener("input",render);$("stage").addEventListener("change",render);$("refresh").addEventListener("click",load);load();
