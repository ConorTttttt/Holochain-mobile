// Real physics/remote rendering evidence, using the explicitly labelled
// development adapters. This does not assert a synchronized playable race.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { connect } from './cdp.mjs';
import { validateMotionEvidence } from './motion-evidence.mjs';
const serial=process.argv[2];
if(!serial?.startsWith('emulator-'))throw new Error('Explicit emulator serial required');
const adb=path.join(process.env.ANDROID_HOME||path.join(process.env.HOME,'Android/Sdk'),'platform-tools/adb');
const runAdb=(...args)=>execFileSync(adb,['-s',serial,...args],{maxBuffer:32*1024*1024,timeout:45000});
assert.equal(runAdb('shell','getprop','ro.kernel.qemu').toString().trim(),'1');
assert.equal(runAdb('emu','avd','name').toString().split('\n')[0].trim(),'TurboKart_API35');
const android=await connect('http://127.0.0.1:9224');
const main=await connect('http://127.0.0.1:9333');
const windowExpression=`process.mainModule.require('electron').BrowserWindow.getAllWindows().find(w=>w.webContents.getURL().startsWith('webhapp://'))`;
const desktop=code=>main.evaluate(`${windowExpression}.webContents.executeJavaScript(${JSON.stringify(code)},true)`);
const key=(type,keyCode)=>main.evaluate(`(()=>{const w=${windowExpression};w.show();w.focus();w.webContents.sendInputEvent(${JSON.stringify({type,keyCode})});return true;})()`);
const delay=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,timeout=20000){const deadline=Date.now()+timeout;while(Date.now()<deadline){if(await fn())return;await delay(100);}throw new Error('Motion-test condition timed out');}
const viewCode=`(()=>{const w=__game.world;const r=w.karts.find(k=>k.netAgent);const projected=r&&r.position.clone().add({x:0,y:.5,z:0}).project(w.chase.camera);let meshes=0;r?.object3D.traverse(o=>{if(o.isMesh&&o.visible)meshes++;});return {phase:__game.state,local:w.player.position.toArray(),remote:r&&{agent:r.netAgent,pos:r.position.toArray(),visible:r.object3D.visible,inScene:r.object3D.parent===w.scene,meshes,projected:projected.toArray()},touch:window.__turboKartTouchState,bridge:window.__turboKartGameBridge||null,motion:window.__desktopMotion&&{received:__desktopMotion.stats.received,rendered:__desktopMotion.stats.rendered,error:__desktopMotion.stats.error,net:__desktopMotion.adapter.stats},errors:__game.errors()};})()`;
const views=async()=>({android:await android.evaluate(viewCode),desktop:await desktop(viewCode)});
const distance=(a,b)=>Math.hypot(...a.map((v,i)=>v-b[i]));
const output='docs/evidence/combined';
const result={developmentOnly:true,sharedRaceTimingAndScoring:false};
try {
  await key('keyUp','Up');
  assert.ok(await desktop(`!!window.__desktopMotion`),'Start the desktop motion adapter before opening the Android admitted game');
  // Run immediately after Android's normal admitted entry. Reuse those original
  // scenes to avoid overlapping old/new GPU allocations on this shared host.
  await until(()=>android.evaluate(`['intro','countdown','racing'].includes(__game.state)&&!!window.__turboKartGameBridge&&!__turboKartGameBridge.error`));
  await Promise.all([desktop(`__game.state==='intro'&&__game.skipIntro()`),android.evaluate(`__game.state==='intro'&&__game.skipIntro()`)]);
  await until(async()=>await android.evaluate(`__game.state==='racing'&&__turboKartTouchState.accelerate`) && await desktop(`__game.state==='racing'`));
  await key('keyDown','Up');
  const dimensions=await android.evaluate(`({w:innerWidth,h:innerHeight})`);
  // Opposing steering touches cancel as in the original touch adapter. The
  // dedicated gameplay suite already proves each direction; keep this motion
  // test on the starting straight so both opponents remain in camera range.
  await android.send('Input.dispatchTouchEvent',{type:'touchStart',touchPoints:[.2,.8].map((x,i)=>({id:i+1,x:dimensions.w*x,y:dimensions.h*.5,radiusX:8,radiusY:8,force:1}))});
  assert.equal(await android.evaluate(`__game.world.player.input.steer`),0);
  await delay(100);await android.send('Input.dispatchTouchEvent',{type:'touchEnd',touchPoints:[]});
  result.initial=await views();
  await delay(1800);
  result.moved=await views();
  for(const side of ['android','desktop']){
    const before=result.initial[side],after=result.moved[side];
    assert.ok(distance(before.local,after.local)>1,`${side} original physics did not move`);
    assert.ok(distance(before.remote.pos,after.remote.pos)>1,`${side} authenticated remote pose did not move`);
    assert.ok(after.remote.visible&&after.remote.inScene&&after.remote.meshes>0,`${side} remote model absent from scene`);
    assert.deepEqual(after.errors,[]);
  }
  assert.ok(result.moved.android.bridge.sent>result.initial.android.bridge.sent);
  assert.ok(result.moved.android.bridge.received>result.initial.android.bridge.received);
  assert.ok(result.moved.android.bridge.rendered>result.initial.android.bridge.rendered);
  assert.equal(result.moved.android.bridge.error,null);
  assert.ok(result.moved.desktop.motion.received>result.initial.desktop.motion.received);
  assert.ok(result.moved.desktop.motion.rendered>result.initial.desktop.motion.rendered);
  assert.equal(result.moved.desktop.motion.error,null);
  const inView=v=>v.remote.projected.every(n=>Math.abs(n)<=1);
  // Use only the original camera control if the moving opponent is behind.
  if(!inView(result.moved.desktop))await key('keyDown','C');
  if(!inView(result.moved.android)){
    const p=await android.evaluate(`(()=>{const r=document.querySelector('#tk-touch-look').getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2};})()`);
    await android.send('Input.dispatchTouchEvent',{type:'touchStart',touchPoints:[{id:2,...p,radiusX:8,radiusY:8,force:1}]});
  }
  await delay(500);
  await until(async()=>{const v=await views();return inView(v.android)&&inView(v.desktop);},5000);
  result.rendered=await views();
  assert.ok(inView(result.rendered.android)&&inView(result.rendered.desktop),'Both original cameras must show their authenticated opponent');
  fs.writeFileSync(path.join(output,'android-multiplayer-motion.png'),runAdb('exec-out','screencap','-p'));
  const screenshot=await main.evaluate(`(async()=>{return(await ${windowExpression}.webContents.capturePage()).toPNG().toString('base64');})()`);
  fs.writeFileSync(path.join(output,'desktop-multiplayer-motion.png'),Buffer.from(screenshot,'base64'));
  result.desktopSession=await desktop(`kartNet.sessionStatus()`);
  result.validation=validateMotionEvidence(result);
  result.passed=true;
  console.log('PASS original physics moves on both devices; authenticated WP3 moves both remote models; interpolation and both renderers active');
  console.log(JSON.stringify({android:result.rendered.android.bridge,desktop:result.rendered.desktop.motion,opponentsInView:{android:inView(result.rendered.android),desktop:inView(result.rendered.desktop)}},null,2));
} catch(error){result.passed=false;result.error=String(error);throw error;}
finally {
  fs.writeFileSync(path.join(output,'multiplayer-motion.json'),JSON.stringify(result,null,2)+'\n');
  try{await key('keyUp','Up');await key('keyUp','C');await android.send('Input.dispatchTouchEvent',{type:'touchEnd',touchPoints:[]});}catch{}
  main.close();android.close();
}
