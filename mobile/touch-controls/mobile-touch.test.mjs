import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { TouchDriveState, CONTROL_KEYS, TAP_MAX_MS } from './mobile-touch.mjs';

test('racing automatically accelerates without touching the screen', () => {
  const calls = [];
  const state = new TouchDriveState((key, down) => calls.push([key, down]));
  assert.deepEqual(state.snapshot(), { left:false, right:false, accelerate:false });
  state.setActive(true);
  assert.deepEqual(state.snapshot(), { left:false, right:false, accelerate:true });
  assert.deepEqual(calls, [[CONTROL_KEYS.accelerate, true]]);
  state.setActive(true);
  assert.equal(calls.length, 1, 'no duplicate throttle keydown');
});

test('left half held steers left; releasing leaves automatic acceleration on', () => {
  const calls = [];
  const state = new TouchDriveState((key, down) => calls.push([key, down]));
  state.setActive(true);
  state.press(1, 'left', 30, 90, 10);
  assert.deepEqual(state.snapshot(), { left:true, right:false, accelerate:true });
  state.release(1, 1010);
  assert.deepEqual(state.snapshot(), { left:false, right:false, accelerate:true });
  assert.deepEqual(calls, [
    [CONTROL_KEYS.accelerate, true], [CONTROL_KEYS.left, true], [CONTROL_KEYS.left, false]
  ]);
});

test('right half held steers right; both halves held cancel steering', () => {
  const state = new TouchDriveState();
  state.setActive(true);
  state.press(1, 'right', 900, 80, 100);
  assert.deepEqual(state.snapshot(), {left:false, right:true, accelerate:true});
  state.press(2, 'left', 80, 80, 110);
  assert.deepEqual(state.snapshot(), {left:false, right:false, accelerate:true});
  state.release(2, 1000);
  assert.deepEqual(state.snapshot(), {left:false, right:true, accelerate:true});
});

test('a quick tap fires weapon once, without interrupting acceleration', () => {
  const fired = [];
  const keys = [];
  const state = new TouchDriveState((key, down) => keys.push([key, down]), () => fired.push('fire'));
  state.setActive(true);
  state.press(11, 'right', 850, 140, 1000);
  state.release(11, 1130);
  assert.deepEqual(fired, ['fire']);
  assert.deepEqual(state.snapshot(), {left:false,right:false,accelerate:true});
  assert.equal(keys.filter(([key,down]) => key === CONTROL_KEYS.accelerate && !down).length, 0);
});

test('long press steers only; does not repeatedly fire', () => {
  let count = 0;
  const state = new TouchDriveState(() => {}, () => count++);
  state.setActive(true);
  state.press(10, 'left', 40, 100, 100);
  state.release(10, 100 + TAP_MAX_MS + 1);
  assert.equal(count, 0);
});

test('sliding a finger across halves changes steering and never fires on release', () => {
  let count = 0;
  const state = new TouchDriveState(() => {}, () => count++);
  state.setActive(true);
  state.press(10, 'left', 100, 100, 5);
  state.move(10, 300, 105, 'right');
  assert.deepEqual(state.snapshot(), {left:false,right:true,accelerate:true});
  state.release(10, 110);
  assert.equal(count, 0);
});

test('swipes within a half do not accidentally fire', () => {
  let count = 0;
  const state = new TouchDriveState(() => {}, () => count++);
  state.setActive(true);
  state.press(1, 'left', 10, 10, 0);
  state.move(1, 80, 90, 'left');
  state.release(1, 100);
  assert.equal(count, 0);
});

test('quick second-finger tap fires while first finger keeps steering', () => {
  let count = 0;
  const state = new TouchDriveState(() => {}, () => count++);
  state.setActive(true);
  state.press(1, 'left', 30, 100, 100);
  state.press(2, 'left', 32, 100, 200);
  state.release(2, 250);
  assert.deepEqual(state.snapshot(), {left:true,right:false,accelerate:true});
  assert.equal(count, 1);
  state.release(1, 1000);
  assert.equal(count, 1);
});

test('cancelled touches, lost capture and backgrounding never fire a weapon', () => {
  let count = 0;
  const keys = [];
  const state = new TouchDriveState((key,down)=>keys.push([key,down]), () => count++);
  state.setActive(true);
  state.press(1, 'left', 100, 100, 100);
  state.release(1, 150, false);
  state.press(2, 'right', 400, 100, 180);
  state.setActive(false);
  state.release(2, 200);
  assert.equal(count, 0);
  assert.deepEqual(state.snapshot(),{left:false,right:false,accelerate:false});
  assert.deepEqual(keys.at(-1), [CONTROL_KEYS.accelerate, false]);
});

test('weapon input uses E rather than Space (which is drift)', () => {
  assert.equal(CONTROL_KEYS.fire, 'KeyE');
  assert.notEqual(CONTROL_KEYS.fire, 'Space');
});

test('inactive controls and invalid steering direction are safe', () => {
  let count = 0;
  const state = new TouchDriveState(() => {}, () => count++);
  state.press(1,'left',10,10,100);
  state.release(1,130);
  assert.equal(count,0);
  state.setActive(true);
  assert.throws(()=>state.press(3,'go',0,0,0));
  state.setActive(false);
  assert.equal(state.activeKeys.size,0);
});
