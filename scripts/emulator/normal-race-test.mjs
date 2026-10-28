// Drive the normal online race using trusted keyboard/touch input only.
// Never sets kart positions, laps, progress, physics inputs or debug autopilot.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { connect } from './cdp.mjs';
const [serial,label='race-2']=process.argv.slice(2);
const renderProbe=process.argv.includes('--render-probe');
if(!serial?.startsWith('emulator-')||!/^race-\d+$/.test(label))throw new Error('Explicit emulator serial and race-N required');
const output='docs/evidence/synchronized';fs.mkdirSync(output,{recursive:true});
const adb=path.join(process.env.ANDROID_HOME||path.join(process.env.HOME,'Android/Sdk'),'platform-tools/adb');
const runAdb=(...args)=>execFileSync(adb,['-s',serial,...args],{maxBuffer:32*1024*1024,timeout:45000});
assert.equal(runAdb('shell','getprop','ro.kernel.qemu').toString().trim(),'1');
assert.equal(runAdb('emu','avd','name').toString().split('\n')[0].trim(),'TurboKart_API35');
const delay=ms=>new Promise(r=>setTimeout(r,ms));
let android;
for(let i=0;i<40;i++){try{android=await connect('http://127.0.0.1:9224');break;}catch{await delay(250);}}
if(!android)throw new Error('Android WebView unavailable');
const main=await connect('http://127.0.0.1:9333');
const windowExpression=`process.mainModule.require('electron').BrowserWindow.getAllWindows().find(w=>w.webContents.getURL().startsWith('webhapp://'))`;
const desktop=code=>main.evaluate(`${windowExpression}.webContents.executeJavaScript(${JSON.stringify(code)},true)`);
const key=(type,keyCode)=>main.evaluate(`(()=>{const w=${windowExpression};w.webContents.sendInputEvent(${JSON.stringify({type,keyCode})});return true;})()`);
const sampleCode=`(()=>{const w=__game.world,n=w?.net,k=w?.player;if(!n)return{state:__game.state,bridge:window.__turboKartOnline,errors:__game.errors()};const p=w.track.getPointAt(((k.trackT||0)+(7+Math.abs(k.speed)*.42)/w.track.length)%1);const error=Math.atan2(Math.sin(Math.atan2(p.x-k.position.x,p.z-k.position.z)-k.heading),Math.cos(Math.atan2(p.x-k.position.x,p.z-k.position.z)-k.heading));return{state:__game.state,autopilot:__game.debug.autopilot,race:n.adapter.raceId,local:n.adapter.localIndex,phase:n.phase,schedule:n.start.schedule,goWallMs:n.goWallMs,clock:n.lastRace?.clock,bridge:window.__turboKartOnline,stats:n.adapter.stats,progress:n.progress.view.body,items:__game.net.items,scheduler:{frameCalls:n.sched.frameCalls,timerCalls:n.sched.timerCalls},player:{pos:k.position.toArray(),heading:k.heading,trackT:k.trackT,speed:k.speed,lap:k.lap,finished:k.finished,item:k.item,itemCount:k.itemCount,error,locked:k.controlsLocked},karts:w.karts.map(k=>({agent:k.netAgent,slot:k.aiSlot,remote:k.remote,pos:k.position.toArray(),projected:k.netAgent&&k.remote?k.object3D.getWorldPosition(k.position.clone()).add({x:0,y:.5,z:0}).project(w.chase.camera).toArray():null,finished:k.finished})),events:window.__normalRaceEvents||[],errors:__game.errors()};})()`;
const observe=`(()=>{if(!window.__normalRaceEvents){window.__normalRaceEvents=[];for(const type of ['race:countdown','race:go','race:lap','race:finish','race:end'])__game.bus.on(type,data=>{if(__normalRaceEvents.length<100)__normalRaceEvents.push({type,wall:Date.now(),page:performance.timeOrigin+performance.now(),n:data?.n,lap:data?.lap,place:data?.place});});}return true;})()`;
const result={label,inputOnly:true,normalOnlineEntry:true,samples:[],fires:[],passed:false};
const state={android:{side:0,acc:0,previous:0},desktop:{side:0,acc:0,previous:0,up:false}};
let dimensions,firePoint;
const point=side=>({id:1,x:dimensions.w*(side<0?.2:.8),y:dimensions.h*.5,radiusX:7,radiusY:7,force:1});
async function touchSide(side){const old=state.android.side;if(side===old)return;await android.send('Input.dispatchTouchEvent',{type:side?(old?'touchMove':'touchStart'):'touchEnd',touchPoints:side?[point(side)]:[]});state.android.side=side;}
async function keyboardSide(side){const old=state.desktop.side;if(side===old)return;if(old)await key('keyUp',old<0?'Left':'Right');if(side)await key('keyDown',side<0?'Left':'Right');state.desktop.side=side;}
function steering(s,held,dt){
  const derivative=Math.atan2(Math.sin(s.player.error-held.previous),Math.cos(s.player.error-held.previous))/Math.max(.02,dt);
  held.previous=s.player.error;
  const target=Math.max(-1,Math.min(1,-s.player.error*2.8-Math.max(-8,Math.min(8,derivative))*.06));
  held.acc+=Math.abs(target);
  if(held.acc>=1){held.acc-=1;return Math.sign(target);}return 0;
}
async function screenshot(name){
  fs.writeFileSync(path.join(output,`${label}-android-${name}.png`),runAdb('exec-out','screencap','-p'));
  const image=await main.evaluate(`(async()=>{return(await ${windowExpression}.webContents.capturePage()).toPNG().toString('base64');})()`);
  fs.writeFileSync(path.join(output,`${label}-desktop-${name}.png`),Buffer.from(image,'base64'));
}
try{
  await main.evaluate(`(()=>{const w=${windowExpression};w.show();w.focus();return true;})()`);
  await Promise.all([android.evaluate(observe),desktop(observe)]);
  dimensions=await android.evaluate(`({w:innerWidth,h:innerHeight})`);
  const started=Date.now(),deadline=started+420000;let last=started,lastSave=0,lastFire=0,countdownShot=false,driveShot=false,probe=null;
  while(Date.now()<deadline){
    const now=Date.now(),dt=(now-last)/1000;last=now;
    const [a,d]=await Promise.all([android.evaluate(sampleCode),desktop(sampleCode)]);
    if(a.bridge?.error)throw new Error(a.bridge.error);
    for(const s of [a,d]){
      if(s.autopilot)throw new Error('Debug autopilot must remain disabled');
      if(s.phase==='failed'||s.phase==='interrupted')throw new Error(`Original race controller ${s.phase}`);
      assert.equal(s.errors.length,0,'Original game error registry');
    }
    if(now-lastSave>=2000){result.samples.push({wall:now,android:a,desktop:d});lastSave=now;fs.writeFileSync(path.join(output,`${label}-live.json`),JSON.stringify(result,null,2)+'\n');console.log(JSON.stringify({seconds:Math.round((now-started)/1000),android:{state:a.state,phase:a.phase,t:a.player?.trackT,speed:a.player?.speed,lap:a.player?.lap,status:a.progress?.status},desktop:{state:d.state,phase:d.phase,t:d.player?.trackT,speed:d.player?.speed,lap:d.player?.lap,status:d.progress?.status}}));}
    if(!countdownShot&&a.state==='countdown'&&d.state==='countdown'){countdownShot=true;await screenshot('countdown');}
    if(a.state==='racing'&&!a.player.finished)await touchSide(steering(a,state.android,dt));else await touchSide(0);
    const driveDesktop=d.state==='racing'&&!d.player.finished;
    if(driveDesktop!==state.desktop.up){await key(driveDesktop?'keyDown':'keyUp','Up');state.desktop.up=driveDesktop;}
    await keyboardSide(driveDesktop?steering(d,state.desktop,dt):0);
    if(!driveShot&&a.stats?.received>20&&d.stats?.received>20&&a.phase==='running'&&d.phase==='running'){
      const visible=s=>s.karts.some(k=>k.agent&&k.remote&&k.projected?.every(n=>Number.isFinite(n)&&Math.abs(n)<=1));
      let looked=false;
      if(!visible(d)){await key('keyDown','C');looked=true;await delay(250);}
      const photo={android:await android.evaluate(sampleCode),desktop:await desktop(sampleCode)};
      if(visible(photo.android)&&visible(photo.desktop)){result.opponentsVisible=photo;driveShot=true;await screenshot('driving');}
      if(looked)await key('keyUp','C');
    }
    if(renderProbe&&!probe&&now-started>22000&&a.phase==='running'){
      const status=await android.evaluate(`__turboKartOnlineSession.api.sessionStatus()`);
      probe={wall:Date.now(),published:status.android_publication.published,received:d.stats.received,scheduler:a.scheduler};
      await android.evaluate(`(()=>{const native=window.__unthrottledRaf=requestAnimationFrame;const times=new WeakMap();window.requestAnimationFrame=callback=>{const step=now=>{if(now-(times.get(callback)||0)>=1000/15){times.set(callback,now);callback(now);}else native(step);};return native(step);};return true;})()`);
    }
    if(probe&&!result.renderProbe&&Date.now()-probe.wall>=10000){
      await android.evaluate(`(()=>{window.requestAnimationFrame=window.__unthrottledRaf;delete window.__unthrottledRaf;return true;})()`);
      const status=await android.evaluate(`__turboKartOnlineSession.api.sessionStatus()`),elapsed=(Date.now()-probe.wall)/1000;
      result.renderProbe={seconds:elapsed,renderFrames:a.scheduler.frameCalls-probe.scheduler.frameCalls,simulationTimerCalls:a.scheduler.timerCalls-probe.scheduler.timerCalls,nativePublished:status.android_publication.published-probe.published,desktopReceived:d.stats.received-probe.received,publicationHz:(status.android_publication.published-probe.published)/elapsed};
      console.log('Render-independence probe',JSON.stringify(result.renderProbe));
    }
    if(now-lastFire>1500&&a.state==='racing'&&a.player?.itemCount>0){
      firePoint=await android.evaluate(`(()=>{const r=document.querySelector('#tk-touch-fire').getBoundingClientRect();if(!r.width||!r.height)throw new Error('FIRE is not visible');return{id:2,x:r.x+r.width/2,y:r.y+r.height/2,radiusX:7,radiusY:7,force:1};})()`);
      await touchSide(0);await android.send('Input.dispatchTouchEvent',{type:'touchStart',touchPoints:[firePoint]});await delay(90);await android.send('Input.dispatchTouchEvent',{type:'touchEnd',touchPoints:[]});
      result.fires.push({side:'android',wall:now,item:a.player.item});lastFire=now;
    }
    if(now-lastFire>1500&&d.state==='racing'&&d.player?.itemCount>0){await key('keyDown','E');await key('keyUp','E');result.fires.push({side:'desktop',wall:now,item:d.player.item});lastFire=now;}
    if(a.progress?.status===3&&d.progress?.status===3){
      result.final={android:a,desktop:d};
      assert.equal(a.race,d.race);assert.deepEqual(a.schedule,d.schedule);
      assert.deepEqual(a.progress.racers,d.progress.racers,'Authoritative final standings must agree');
      assert.equal(a.progress.racers[a.local].state,1,'Android player must finish');
      assert.equal(d.progress.racers[d.local].state,1,'Desktop player must finish');
      assert.ok(result.opponentsVisible,'Both original cameras must have displayed their authenticated opponent');
      if(renderProbe){assert.ok(result.renderProbe,'Rendering probe did not run');assert.ok(result.renderProbe.publicationHz>=19&&result.renderProbe.publicationHz<=21,'Native publication must remain nominal 20 Hz during 15 FPS rendering');assert.ok(result.renderProbe.nativePublished>result.renderProbe.renderFrames,'Publication must not depend on rendered frames');}
      result.native=await android.evaluate(`__turboKartOnlineSession.api.sessionStatus()`);
      result.publicationMetrics=await android.evaluate(`__turboKartOnlineSession.rpc.request('publication_metrics')`);
      result.desktopSession=await desktop(`kartNet.sessionStatus()`);
      await screenshot('finished');result.passed=true;
      console.log('PASS normal synchronized race completed with matching authoritative standings');break;
    }
    await delay(Math.max(0,50-(Date.now()-now)));
  }
  assert.ok(result.passed,'Complete normal race timed out');
}catch(error){result.error=String(error);try{await screenshot('failure');}catch{}throw error;}
finally{
  fs.writeFileSync(path.join(output,`${label}-result.json`),JSON.stringify(result,null,2)+'\n');
  try{await key('keyUp','Up');await key('keyUp','C');await keyboardSide(0);await touchSide(0);await android.evaluate(`(()=>{if(window.__unthrottledRaf){requestAnimationFrame=window.__unthrottledRaf;delete window.__unthrottledRaf;}return true;})()`);}catch{}
  main.close();android.close();
}
