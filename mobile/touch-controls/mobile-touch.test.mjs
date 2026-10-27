import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { TouchDriveState, CONTROL_KEYS } from './mobile-touch.mjs';

const empty = { left: false, right: false, accelerate: false, lookBack: false };

test('starts with auto-acceleration, and no touch is needed', () => {
  const keys = [];
  const input = new TouchDriveState((key, down) => keys.push([key, down]));
  assert.deepEqual(input.snapshot(), empty);
  input.setActive(true);
  assert.deepEqual(input.snapshot(), { ...empty, accelerate: true });
  assert.deepEqual(keys, [[CONTROL_KEYS.accelerate, true]]);
  input.setActive(true);
  assert.equal(keys.length, 1, 'holding throttle does not generate duplicate keydowns');
});

test('the entire left and right halves steer, without affecting throttle', () => {
  const keys = [];
  const input = new TouchDriveState((key, down) => keys.push([key, down]));
  input.setActive(true);
  input.press(10, 'left');
  assert.deepEqual(input.snapshot(), { ...empty, accelerate: true, left: true });
  input.release(10);
  assert.deepEqual(input.snapshot(), { ...empty, accelerate: true });
  input.press(11, 'right');
  assert.deepEqual(input.snapshot(), { ...empty, accelerate: true, right: true });
  input.release(11);
  assert.deepEqual(input.snapshot(), { ...empty, accelerate: true });
  assert.equal(keys.filter(([key, down]) => key === 'ArrowUp' && !down).length, 0);
});

test('moving across the midpoint changes steering, never fires the weapon', () => {
  let fired = 0;
  const input = new TouchDriveState(() => {}, () => fired++);
  input.setActive(true);
  input.press(1, 'left');
  input.move(1, 'right');
  assert.deepEqual(input.snapshot(), { ...empty, accelerate: true, right: true });
  input.release(1);
  assert.equal(fired, 0);
});

test('quick steering taps and prolonged steering never fire any item', () => {
  let fired = 0;
  const input = new TouchDriveState(() => {}, () => fired++);
  input.setActive(true);
  input.press(1, 'left');
  input.release(1);
  input.press(2, 'right');
  input.release(2);
  assert.equal(fired, 0);
});

test('the separate fire button action sends an event only while racing', () => {
  let fired = 0;
  const input = new TouchDriveState(() => {}, () => fired++);
  input.fireWeapon();
  assert.equal(fired, 0);
  input.setActive(true);
  input.fireWeapon();
  assert.equal(fired, 1);
  input.setActive(false);
  input.fireWeapon();
  assert.equal(fired, 1);
  assert.equal(CONTROL_KEYS.fire, 'KeyE');
});

test('holding Look Back maps to C and releasing restores the camera', () => {
  const keys = [];
  const input = new TouchDriveState((key, down) => keys.push([key, down]));
  input.setActive(true);
  input.pressLook(12);
  assert.deepEqual(input.snapshot(), { ...empty, accelerate: true, lookBack: true });
  input.releaseLook(12);
  assert.deepEqual(input.snapshot(), { ...empty, accelerate: true });
  assert.deepEqual(keys.slice(-2), [['KeyC', true], ['KeyC', false]]);
  assert.equal(CONTROL_KEYS.lookBack, 'KeyC');
});

test('Look Back and steering work simultaneously and independently', () => {
  const input = new TouchDriveState();
  input.setActive(true);
  input.press(1, 'left');
  input.pressLook(2);
  assert.deepEqual(input.snapshot(), { ...empty, accelerate: true, left: true, lookBack: true });
  input.releaseLook(2);
  assert.deepEqual(input.snapshot(), { ...empty, accelerate: true, left: true });
  input.release(1);
  assert.deepEqual(input.snapshot(), { ...empty, accelerate: true });
});

test('multiple look-back fingers keep camera backwards until the last lifts', () => {
  const keys = [];
  const input = new TouchDriveState((key, down) => keys.push([key, down]));
  input.setActive(true);
  input.pressLook(1);
  input.pressLook(2);
  input.releaseLook(1);
  assert.equal(input.snapshot().lookBack, true);
  input.releaseLook(2);
  assert.equal(input.snapshot().lookBack, false);
  assert.deepEqual(keys.filter(([key]) => key === 'KeyC'), [['KeyC', true], ['KeyC', false]]);
});

test('opposite steering halves cancel; Look Back remains active', () => {
  const input = new TouchDriveState();
  input.setActive(true);
  input.press(1, 'left');
  input.press(2, 'right');
  input.pressLook(3);
  assert.deepEqual(input.snapshot(), { ...empty, accelerate: true, lookBack: true });
  input.release(1);
  assert.deepEqual(input.snapshot(), { ...empty, accelerate: true, right: true, lookBack: true });
});

test('pause/background releases throttle, look-back and steering without firing', () => {
  const keys = [];
  let fired = 0;
  const input = new TouchDriveState((key, down) => keys.push([key, down]), () => fired++);
  input.setActive(true);
  input.press(1, 'left');
  input.pressLook(2);
  input.setActive(false);
  assert.deepEqual(input.snapshot(), empty);
  assert.equal(input.activeKeys.size, 0);
  assert.equal(input.pointers.size, 0);
  assert.equal(input.lookPointers.size, 0);
  assert.equal(fired, 0);
  for (const key of ['ArrowUp', 'ArrowLeft', 'KeyC']) {
    assert.equal(keys.filter(([code, down]) => code === key && down === false).length, 1);
  }
  input.setActive(true);
  assert.deepEqual(input.snapshot(), { ...empty, accelerate: true });
});

test('invalid steering side is rejected and inactive touches do nothing', () => {
  const input = new TouchDriveState();
  input.press(1, 'left');
  input.pressLook(1);
  assert.deepEqual(input.snapshot(), empty);
  input.setActive(true);
  assert.throws(() => input.press(5, 'go'));
  input.press(1, 'left');
  assert.throws(() => input.move(1, 'down'));
});
