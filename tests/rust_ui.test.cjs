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
    scored_count:10,quote_count:1900,scope:'固定十檔',model_version:'test',
    candidates:Array.from({length:10},(_,i)=>({symbol:String(2330+i),name:i?'測試股':'<img src=x onerror=alert(1)>',market:'TWSE',stage:i?'觀察':'融資風險',score:50,close:100,price_change_pct:-1,balance_lots:1000,change_lots:20,utilization_pct:null,deleveraging_pct:-10,breakdown:{'條件':50},quota_basis:'代理'})),
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
  p.$('search').value='2330';p.$('search').events.input();
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
