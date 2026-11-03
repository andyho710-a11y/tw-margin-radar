// Run in the visible wmux browser with: wmux browser eval <this source>.
// Tests actual visible browser interaction and 320/390 CSS-pixel content layouts.
// Records the browser viewport separately; this is not physical device emulation.
window.__radarMobileProbe={phase:"starting"};
void(async()=>{
  if(location.origin!=="http://127.0.0.1:8765")throw Error("local preview required");
  const pause=ms=>new Promise(r=>setTimeout(r,ms));
  const wait=async(fn)=>{for(let i=0;i<100;i++){if(fn())return;await pause(50);}throw Error("preview did not settle");};
  const win=window,doc=document;
  await wait(()=>doc.querySelectorAll(".stock").length===10&&!doc.getElementById("refresh").disabled);
  window.__radarMobileProbe={phase:"layout"};
  const expected=["5274","2059","3443","3653","3008","7769","6515","2383","6223","2454"];
  const codes=()=>[...doc.querySelectorAll(".stock h3")].map(e=>e.textContent.split(" ")[0]);
  const exact=values=>values.length===10&&new Set(values).size===10&&expected.every(c=>values.includes(c));
  const result={fixed_ten_exact:exact(codes()),browser_viewport_width:innerWidth,layouts:[]};
  for(const width of [320,390]){
    doc.documentElement.style.width=width+"px";await pause(100);
    const box=doc.documentElement.getBoundingClientRect();
    const overflow=[...doc.querySelectorAll(".stock,.summary article,.panel,header,footer")].filter(e=>e.getBoundingClientRect().right>box.right+1).map(e=>e.className||e.tagName);
    result.layouts.push({requested_content_width:width,actual_content_width:box.width,no_content_overflow:overflow.length===0,overflow_elements:overflow,columns:win.getComputedStyle(doc.getElementById("stocks")).gridTemplateColumns.split(" ").length});
  }
  doc.documentElement.style.width="390px";
  const search=doc.getElementById("search"),stage=doc.getElementById("stage");
  search.value="7769";search.dispatchEvent(new win.Event("input",{bubbles:true}));
  result.search_7769=codes().length===1&&codes()[0]==="7769";
  doc.querySelector("summary").click();result.details_expand=doc.querySelector("details").open;
  doc.getElementById("refresh").click();await wait(()=>!doc.getElementById("refresh").disabled);
  result.refresh_keeps_filter=codes().length===1&&codes()[0]==="7769";
  search.value="";search.dispatchEvent(new win.Event("input",{bubbles:true}));
  const stageName=doc.querySelector(".tag").textContent;
  const expectedCount=[...doc.querySelectorAll(".tag")].filter(e=>e.textContent===stageName).length;
  stage.value=stageName;stage.dispatchEvent(new win.Event("change",{bubbles:true}));result.stage_filter=codes().length===expectedCount;
  stage.value="";stage.dispatchEvent(new win.Event("change",{bubbles:true}));
  const original=win.fetch.bind(win),valid=await(await original("data/latest.json")).json();
  const bad=structuredClone(valid);bad.candidates[0].symbol="2330";
  win.fetch=async()=>new win.Response(JSON.stringify(bad),{status:200,headers:{"Content-Type":"application/json"}});
  doc.getElementById("refresh").click();await wait(()=>!doc.getElementById("refresh").disabled);
  result.wrong_ten_hidden=codes().length===0&&doc.getElementById("status").textContent.includes("錯誤");
  const full=await(await original("data/all-market.json")).json();
  win.fetch=async()=>new win.Response(JSON.stringify(full),{status:200,headers:{"Content-Type":"application/json"}});
  doc.getElementById("refresh").click();await wait(()=>!doc.getElementById("refresh").disabled);
  result.all_market_outside_fixed_list=codes().length===10&&codes().some(c=>!expected.includes(c))&&doc.getElementById("scope").textContent.includes("全市場獨立掃描");
  win.fetch=async()=>{throw Error("offline test");};doc.getElementById("refresh").click();await wait(()=>!doc.getElementById("refresh").disabled);
  result.failed_refresh_clears=codes().length===0&&doc.getElementById("sources").children.length===0;
  win.fetch=original;doc.getElementById("refresh").click();await wait(()=>!doc.getElementById("refresh").disabled);
  result.recovery_exact_ten=exact(codes());result.stale_warning=doc.getElementById("status").textContent.includes("交易日已超過");
  result.passed=Object.entries(result).filter(([k])=>!["layouts","browser_viewport_width"].includes(k)).every(([,v])=>v===true)&&result.layouts.every(v=>v.no_content_overflow&&v.actual_content_width===v.requested_content_width);
  window.scrollTo(0,0);
  return result;
})().then(result=>{window.__radarMobileProbe=result;}).catch(error=>{window.__radarMobileProbe={passed:false,error:String(error)};});
"started"
