// Compare actual GO events on one observer's monotonic clock. Emulator wall
// time can differ from the host; subtracting two Date.now() values is invalid.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { connect } from './cdp.mjs';
const label=process.argv[2];
assert.match(label||'',/^race-\d+$/);
const android=await connect('http://127.0.0.1:9224');
const main=await connect('http://127.0.0.1:9333');
const code=`(()=>{const n=__game.world.net;return{now:performance.now(),origin:performance.timeOrigin,go:n.goWallMs,lateMs:n.lateMs,schedule:n.start.schedule,clock:n.lastRace.clock};})()`;
const desktop=()=>main.evaluate(`process.mainModule.require('electron').BrowserWindow.getAllWindows().find(w=>w.webContents.getURL().startsWith('webhapp://')).webContents.executeJavaScript(${JSON.stringify(code)},true)`);
const result={label,method:'CDP midpoint maps page monotonic GO to observer monotonic time; no cross-device wall-clock subtraction',samples:{android:[],desktop:[]}};
try{
  for(let i=0;i<8;i++)for(const [side,read] of [['android',()=>android.evaluate(code)],['desktop',desktop]]){
    const before=performance.now(),page=await read(),after=performance.now();
    assert.ok(Number.isFinite(page.go));
    result.samples[side].push({page,rttMs:after-before,observerGoMs:(before+after)/2-(page.now-(page.go-page.origin))});
  }
  const best=side=>result.samples[side].reduce((a,b)=>a.rttMs<b.rttMs?a:b);
  const a=best('android'),d=best('desktop');
  assert.deepEqual(a.page.schedule,d.page.schedule);
  result.goDifferenceMs=Math.abs(a.observerGoMs-d.observerGoMs);
  result.observationUncertaintyMs=(a.rttMs+d.rttMs)/2;
  result.androidLateMs=a.page.lateMs;result.desktopLateMs=d.page.lateMs;
  assert.ok(result.goDifferenceMs<=60+result.observationUncertaintyMs,'Actual GO disagreement exceeds the original Internet clock tier');
  result.passed=true;
  console.log(JSON.stringify({passed:true,goDifferenceMs:result.goDifferenceMs,observationUncertaintyMs:result.observationUncertaintyMs,androidLateMs:result.androidLateMs,desktopLateMs:result.desktopLateMs}));
}finally{
  fs.writeFileSync(`docs/evidence/synchronized/${label}-clock-validation.json`,JSON.stringify(result,null,2)+'\n');
  android.close();main.close();
}
