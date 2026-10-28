// Real WebView/renderer tests. CDP multi-pointer touch events are used together,
// not sequential adb taps. Start Single Player through MainActivity beforehand.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { connect } from './cdp.mjs';
const [serial, endpoint = 'http://127.0.0.1:9224', output = 'docs/evidence/combined'] = process.argv.slice(2);
if (!serial?.startsWith('emulator-')) throw new Error('Explicit emulator serial required');
const adb = path.join(process.env.ANDROID_HOME || path.join(process.env.HOME, 'Android/Sdk'), 'platform-tools/adb');
const runAdb = (...args) => execFileSync(adb, ['-s', serial, ...args], { maxBuffer: 32 * 1024 * 1024 });
assert.equal(runAdb('shell', 'getprop', 'ro.kernel.qemu').toString().trim(), '1');
fs.mkdirSync(output, { recursive: true });
const client = await connect(endpoint);
const results = [];
const delay = ms => new Promise(r => setTimeout(r, ms));
const save = () => fs.writeFileSync(path.join(output, 'emulator-gameplay.json'), JSON.stringify(results, null, 2));
const record = (name, value) => { results.push({ name, passed: true, value }); console.log(`PASS ${name}`); save(); };
const evaluate = expression => client.evaluate(expression);
async function until(expression, accepts = Boolean, timeout = 45000) {
  const deadline = Date.now() + timeout; let value;
  while (Date.now() < deadline) { value = await evaluate(expression); if (accepts(value)) return value; await delay(100); }
  throw new Error(`Timed out: ${expression}; last=${JSON.stringify(value)}`);
}
const shot = async name => { await delay(450); await evaluate(`new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`); fs.writeFileSync(path.join(output, name + '.png'), runAdb('exec-out', 'screencap', '-p')); };
const click = selector => evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
async function touch(type, points) { await client.send('Input.dispatchTouchEvent', { type, touchPoints: points.map(p => ({ radiusX: 8, radiusY: 8, force: 1, ...p })) }); }
const state = () => evaluate(`({phase:__game.state,countdownTime:__game.world.race?.countdownTime,touch:window.__turboKartTouchState,input:__game.world.player?.input,position:__game.world.player?.position.toArray(),heading:__game.world.player?.heading,cameraLookBack:__game.world.chase?.lookBack,speed:__game.world.player?.speed,boost:__game.world.player?.boostSource,boostTimer:__game.world.player?.boostTimer,stallTimer:__game.world.player?.stallTimer,errors:__game.errors()})`);
try {
  try { await touch('touchCancel', []); } catch (error) { if (!String(error).includes('Must send a TouchStart')) throw error; }
  await until(`!!window.__game?.world?.track`);
  if (await evaluate(`__game.state !== 'title'`)) await evaluate(`__game.goToTitle()`);
  await until(`__game.state === 'title'`);
  const rendering = await evaluate(`(()=>{const gl=__game.renderer.getContext();return {version:gl.getParameter(gl.VERSION),renderer:gl.getParameter(gl.RENDERER),width:innerWidth,height:innerHeight,karts:__game.world.karts.length,errors:__game.errors()};})()`);
  assert.ok(rendering.karts > 0); assert.ok(rendering.width > rendering.height); assert.equal(rendering.errors.length, 0);
  record('original title, track, karts and landscape WebGL', rendering); await shot('single-player-title');
  await click('[data-action="single-player"]');
  await until(`__game.state === 'select'`); await shot('single-player-menu'); record('original single-player selection menu', true);
  await evaluate(`document.querySelector('.select-screen .race-btn').click()`);
  await until(`['intro','countdown','racing'].includes(__game.state)`); await shot('track-loaded');
  await evaluate(`(()=>{window.__tkEvidenceDetach?.(); const handlers=[];window.__tkControlEvidence=[];window.__tkKeyEvidence=[];for(const name of ['pointerdown','pointerup','pointercancel']){const fn=e=>__tkControlEvidence.push({name,id:e.pointerId,type:e.pointerType,target:e.target.id,trusted:e.isTrusted});document.addEventListener(name,fn,true);handlers.push([document,name,fn]);}for(const name of ['keydown','keyup']){const fn=e=>__tkKeyEvidence.push({name,code:e.code});window.addEventListener(name,fn,true);handlers.push([window,name,fn]);}window.__tkEvidenceDetach=()=>handlers.forEach(([target,name,fn])=>target.removeEventListener(name,fn,true));})()`);
  await evaluate(`__game.skipIntro()`);
  await until(`__game.state === 'countdown'`);
  const before = await state(); assert.equal(before.touch.accelerate, false);
  record('no automatic acceleration before GO', before);
  // The original countdown advances with simulation dt, while its boost hold
  // uses wall time. Touch late in the real countdown so slow emulator frames
  // do not turn the test's early hold into an intentional original-game stall.
  await until(`__game.state === 'countdown' && __game.world.race.countdownTime >= 3.2`, Boolean, 12000);
  const dimensions = await evaluate(`({w:innerWidth,h:innerHeight})`);
  const left = { id: 1, x: dimensions.w * .2, y: dimensions.h * .5 };
  const right = { id: 2, x: dimensions.w * .8, y: dimensions.h * .5 };
  await touch('touchStart', [right]);
  await until(`__turboKartTouchState.accelerate === true`);
  await until(`__game.state === 'racing'`, Boolean, 6000);
  const boost = await state(); assert.equal(boost.boost, 'start'); assert.ok(boost.boostTimer > 0);
  await touch('touchEnd', []); await delay(300);
  assert.equal((await state()).touch.accelerate, true); record('manual countdown start boost and automatic handoff', boost);
  const initial = await state(); await delay(500); const moved = await state();
  assert.ok(moved.position.some((v, i) => Math.abs(v - initial.position[i]) > .01)); record('actual original physics advances player', { initial, moved }); await shot('kart-visible');
  await touch('touchStart', [left]); await delay(180); const steeringLeft = await state();
  assert.equal(steeringLeft.touch.left, true); assert.ok(steeringLeft.input.steer < 0);
  await touch('touchEnd', []); await delay(150);
  await touch('touchStart', [right]); await delay(180); const steeringRight = await state();
  assert.equal(steeringRight.touch.right, true); assert.ok(steeringRight.input.steer > 0);
  await touch('touchEnd', []); record('left and right touch steer original input', { steeringLeft, steeringRight });
  const look = await evaluate(`(()=>{const r=document.querySelector('#tk-touch-look').getBoundingClientRect();return {id:3,x:r.x+r.width/2,y:r.y+r.height/2};})()`);
  await touch('touchStart', [left, look]); await delay(180);
  const multiple = await state(); assert.equal(multiple.touch.left, true); assert.equal(multiple.touch.lookBack, true); assert.equal(multiple.input.lookBack, true); assert.equal(multiple.cameraLookBack, true);
  const pointers = await evaluate(`__tkControlEvidence.filter(e=>e.name==='pointerdown').slice(-2)`);
  assert.equal(new Set(pointers.map(p => p.id)).size, 2); assert.ok(pointers.every(p=>p.type==='touch' && p.trusted));
  record('simultaneous steering and LOOK BACK pointers', { multiple, pointers }); await shot('touch-controls');
  await touch('touchEnd', []); await delay(180); assert.equal((await state()).input.lookBack, false); assert.equal((await state()).cameraLookBack, false); record('LOOK BACK release restores forward input', true);
  await evaluate(`(()=>{const k=__game.world.player;k.item='rocket_booster';k.itemCount=1;window.__fireCount=0;const use=__game.world.items._useItem.bind(__game.world.items);__game.world.items._useItem=(kart,...rest)=>{if(kart===k)__fireCount++;return use(kart,...rest)};})()`);
  const fire = await evaluate(`(()=>{const r=document.querySelector('#tk-touch-fire').getBoundingClientRect();return {id:4,x:r.x+r.width/2,y:r.y+r.height/2};})()`);
  await touch('touchStart', [fire]); await delay(220); await touch('touchEnd', []);
  const fired = await evaluate(`({count:__fireCount,item:__game.world.player.item,timer:__game.world.player.rocketSteerTimer})`);
  assert.equal(fired.count, 1); assert.ok(fired.timer > 0); record('FIRE invokes original item system exactly once', fired);
  await shot('gameplay-running');
  await click('#tk-touch-pause'); await until(`__game.state === 'paused'`); assert.equal((await state()).touch.accelerate, false); await shot('paused');
  await click('.pause-screen [data-a="resume"]'); await until(`__game.state === 'racing' && __turboKartTouchState.accelerate`); record('touch pause and resume', true);
  await touch('touchStart', [left, look]); await delay(120);
  runAdb('shell', 'input', 'keyevent', 'KEYCODE_HOME'); await delay(500);
  const hidden = await state(); assert.equal(hidden.touch.left, false); assert.equal(hidden.touch.lookBack, false); assert.equal(hidden.touch.accelerate, false);
  record('Android background clears all held inputs', hidden);
  // Resume the existing task without targeting the private game Activity.
  runAdb('shell', 'am', 'start', '-n', 'dev.holochain.turbokart.mobile/.MainActivity');
  await delay(300);
  await until(`document.visibilityState === 'visible'`);
  try { await touch('touchCancel', []); } catch (error) { if (!String(error).includes('Must send a TouchStart')) throw error; }
  if (await evaluate(`__game.state === 'paused'`)) await click('.pause-screen [data-a="resume"]');
  await until(`__game.state === 'racing' && __turboKartTouchState.accelerate`);
  record('Android task resumes with automatic throttle and released steering/camera', await state());
  await click('#tk-touch-pause');
  await until(`__game.state === 'paused'`);
  await click('.pause-screen [data-a="quit"]');
  await until(`__game.state === 'title'`); await shot('returned-menu');
  record('original pause menu returns to title', true);
  const errors = await evaluate(`__game.errors()`); assert.equal(errors.length, 0); record('game error registry empty', errors);
} catch (error) {
  results.push({ name: 'failure', passed: false, error: String(error) }); save();
  try { await shot('failure'); } catch {}
  throw error;
} finally { try { await evaluate(`window.__tkEvidenceDetach?.()`); } catch {} client.close(); }
