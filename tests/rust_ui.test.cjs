const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const script = fs.readFileSync(path.join(__dirname, '../docs/rust/radar.js'), 'utf8');

function documentStub() {
  const elements = new Map();
  function element() {
    return {value:'',innerHTML:'',textContent:'',hidden:false,disabled:false,children:[],events:{},
      addEventListener(name, handler){this.events[name]=handler;},
      replaceChildren(){this.children=[];},append(...items){this.children.push(...items);}};
  }
  return {elements,getElementById(id){if(!elements.has(id))elements.set(id,element());return elements.get(id);},createElement:element,createTextNode:text=>({text})};
}
function report() {
  return {schema_version:1,date:new Date(Date.now()+8*3600000).toISOString().slice(0,10),
    generated_at:new Date().toISOString(),data_observed_at:new Date().toISOString(),historical_import:false,
    scored_count:10,quote_count:1900,scope:'固定十檔',model_version:'test',universe_mode:'fixed',
    fixed_watchlist:['5274','2059','3443','3653','3008','7769','6515','2383','6223','2454'],
    candidates:['5274','2059','3443','3653','3008','7769','6515','2383','6223','2454'].map((symbol,i)=>({symbol,name:i?'測試股':'<img src=x onerror=alert(1)>',market:['5274','6223'].includes(symbol)?'TPEX':'TWSE',stage:i?'觀察':'融資風險',score:50,close:100,price_change_pct:-1,balance_lots:1000,change_lots:20,utilization_pct:null,deleveraging_pct:-10,breakdown:{'條件':50},quota_basis:'代理'})),
    sources:{official:{url:'https://www.twse.com.tw/exchangeReport/MI_MARGN',sha256:'a'.repeat(64)},unsafe:{url:'https://example.com/secret',sha256:'x'}}};
}
async function page(fetch) {
  const document=documentStub();
  const ctx=vm.createContext({document,fetch,URL,Date,console});
  vm.runInContext(script,ctx);
  await new Promise(resolve=>setImmediate(resolve));
  return {ctx,document,$:id=>document.getElementById(id)};
}

test('complete report escapes names, filters by code/stage and permits official source links', async()=>{
  const d=report(),p=await page(async()=>({ok:true,json:async()=>d}));
  assert.equal((p.$('stocks').innerHTML.match(/class="stock"/g)||[]).length,10);
  assert.match(p.$('stocks').innerHTML,/&lt;img/);
  assert.doesNotMatch(p.$('stocks').innerHTML,/<img/);
  assert.equal(p.$('sources').children.length,1);
  p.$('search').value='5274';p.$('search').events.input();
  assert.equal((p.$('stocks').innerHTML.match(/class="stock"/g)||[]).length,1);
  p.$('stage').value='去槓桿後轉強';p.$('stage').events.change();assert.equal(p.$('empty').hidden,false);
});
test('failed refresh clears previous ranking and source metadata',async()=>{
  const d=report();let fail=false;
  const p=await page(async()=>{if(fail)throw Error('offline');return {ok:true,json:async()=>d};});
  fail=true;await p.$('refresh').events.click();
  assert.equal(p.$('stocks').innerHTML,'');assert.equal(p.$('sources').children.length,0);assert.equal(p.$('date').textContent,'—');assert.match(p.$('status').textContent,/尚無可用/);
});
test('partial or future reports are rejected',async()=>{
  for(const change of [d=>d.candidates.pop(),d=>d.generated_at='2099-01-01T00:00:00Z',d=>d.date='2026-02-30',d=>d.data_observed_at='2099-01-01T00:00:00Z']){
    const d=report();change(d);const p=await page(async()=>({ok:true,json:async()=>d}));assert.equal(p.$('stocks').innerHTML,'');assert.match(p.$('status').textContent,/尚無可用/);
  }
});
test('old sessions and historical imports show a warning',async()=>{
  const d=report();d.date='2020-01-01';d.historical_import=true;const p=await page(async()=>({ok:true,json:async()=>d}));
  assert.match(p.$('status').className,/warn/);assert.match(p.$('status').textContent,/歷史補抓/);assert.match(p.$('status').textContent,/交易日已超過/);
});
test('fixed mode rejects a wrong ten, duplicate or an obsolete configured list',async()=>{
  for(const change of [d=>d.candidates[0].symbol='2330',d=>d.candidates[0]=d.candidates[1],d=>d.fixed_watchlist[0]='2330']){
    const d=report();change(d);const p=await page(async()=>({ok:true,json:async()=>d}));assert.equal(p.$('stocks').innerHTML,'');assert.match(p.$('status').textContent,/不符|重複|錯誤/);
  }
});
test('all-market mode can display stocks outside the fixed ten',async()=>{
  const d=report();d.universe_mode='all_market';d.candidates[0].symbol='2330';d.candidates[0].market='TWSE';
  const p=await page(async()=>({ok:true,json:async()=>d}));assert.match(p.$('stocks').innerHTML,/2330/);assert.match(p.$('scope').textContent,/全市場獨立掃描/);
});
