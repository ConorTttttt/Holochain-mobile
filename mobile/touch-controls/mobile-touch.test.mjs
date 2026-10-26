import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { TouchKeyState } from './mobile-touch.mjs';

test('left touch and GO touch accelerate simultaneously', () => {
  const calls = [];
  const t = new TouchKeyState((code, down) => calls.push([code, down]));
  t.press(1, 'left');
  t.press(2, 'go');
  assert.deepEqual(t.snapshot(), {left:true, right:false, go:true});
  assert.deepEqual(calls, [['ArrowLeft',true],['ArrowUp',true]]);
  t.release(1);
  assert.deepEqual(t.snapshot(), {left:false, right:false, go:true});
  assert.deepEqual(calls.at(-1), ['ArrowLeft',false]);
  t.release(2);
  assert.deepEqual(calls.at(-1), ['ArrowUp',false]);
});

test('right touch and GO touch accelerate simultaneously', () => {
  const t = new TouchKeyState();
  t.press(6, 'right'); t.press(7, 'go');
  assert.deepEqual(t.snapshot(), {left:false, right:true, go:true});
});

test('left and right simultaneously cancel steering without releasing GO', () => {
  const calls = [];
  const t = new TouchKeyState((key,down)=>calls.push([key,down]));
  t.press(1,'left'); t.press(2,'go'); t.press(3,'right');
  assert.deepEqual(t.snapshot(), {left:false,right:false,go:true});
  assert.deepEqual(calls.at(-1), ['ArrowLeft',false]);
  t.release(3);
  assert.deepEqual(t.snapshot(), {left:true,right:false,go:true});
});

test('release all on blur/pause and no repeated keydowns', () => {
  const calls=[]; const t=new TouchKeyState((key,down)=>calls.push([key,down]));
  t.press(1,'left'); t.press(1,'left'); t.press(2,'go');
  assert.equal(calls.length, 2);
  t.releaseAll();
  assert.equal(calls.length,4);
  assert.deepEqual(t.snapshot(),{left:false,right:false,go:false});
});

test('unknown action cannot silently send a keyboard event', () => {
  const t = new TouchKeyState();
  assert.throws(()=>t.press(9,'brake'));
});
