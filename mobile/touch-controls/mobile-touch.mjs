// Turbo Kart landscape touchscreen controls.
//
// Automatically accelerate during races; transparent left/right halves steer.
// The separate FIRE button sends E, and holding LOOK BACK sends C.
// Preserve the game's native weapon/camera logic, race states and desktop keys.
export const CONTROL_KEYS = Object.freeze({
  left: 'ArrowLeft',
  right: 'ArrowRight',
  accelerate: 'ArrowUp',
  fire: 'KeyE',
  lookBack: 'KeyC',
});

export class TouchDriveState {
  constructor(onKeyChange = () => {}, onFire = () => {}) {
    this.onKeyChange = onKeyChange;
    this.onFire = onFire;
    this.active = false;
    this.pointers = new Map();
    this.lookPointers = new Set();
    this.activeKeys = new Set();
  }

  setActive(active) {
    this.active = Boolean(active);
    if (!this.active) {
      this.pointers.clear();
      this.lookPointers.clear();
    }
    this.sync();
  }

  press(pointerId, side) {
    if (!this.active) return;
    if (side !== 'left' && side !== 'right') throw new Error('Unknown steering side');
    if (this.pointers.has(pointerId)) return;
    this.pointers.set(pointerId, side);
    this.sync();
  }

  move(pointerId, side) {
    if (!this.pointers.has(pointerId)) return;
    if (side !== 'left' && side !== 'right') throw new Error('Unknown steering side');
    if (this.pointers.get(pointerId) !== side) {
      this.pointers.set(pointerId, side);
      this.sync();
    }
  }

  release(pointerId) {
    if (!this.pointers.delete(pointerId)) return;
    this.sync();
  }

  pressLook(pointerId) {
    if (!this.active || this.lookPointers.has(pointerId)) return;
    this.lookPointers.add(pointerId);
    this.sync();
  }

  releaseLook(pointerId) {
    if (!this.lookPointers.delete(pointerId)) return;
    this.sync();
  }

  fireWeapon() {
    if (this.active) this.onFire();
  }

  releaseAll() {
    this.pointers.clear();
    this.lookPointers.clear();
    this.sync();
  }

  snapshot() {
    const sides = new Set(this.pointers.values());
    return {
      left: this.active && sides.has('left') && !sides.has('right'),
      right: this.active && sides.has('right') && !sides.has('left'),
      accelerate: this.active,
      lookBack: this.active && this.lookPointers.size > 0,
    };
  }

  sync() {
    const s = this.snapshot();
    const next = new Set();
    if (s.left) next.add(CONTROL_KEYS.left);
    if (s.right) next.add(CONTROL_KEYS.right);
    if (s.accelerate) next.add(CONTROL_KEYS.accelerate);
    if (s.lookBack) next.add(CONTROL_KEYS.lookBack);
    for (const key of this.activeKeys) {
      if (!next.has(key)) this.onKeyChange(key, false);
    }
    for (const key of next) {
      if (!this.activeKeys.has(key)) this.onKeyChange(key, true);
    }
    this.activeKeys = next;
  }
}

export function attachTurboKartTouchControls(win = window, doc = document) {
  if (!doc.body) throw new Error('Wait for DOMContentLoaded before adding touch controls');
  if (win.__turboKartTouchDetach) return win.__turboKartTouchDetach;

  const style = doc.createElement('style');
  style.textContent = `
    #tk-touch-controls {
      position:fixed; inset:0; z-index:9990; display:none;
      pointer-events:none; touch-action:none; -webkit-user-select:none;
      user-select:none; font-family:system-ui,sans-serif;
    }
    #tk-touch-controls.tk-active { display:block; }
    #tk-touch-controls .tk-half {
      position:absolute; top:0; bottom:0; width:50%;
      box-sizing:border-box; pointer-events:auto; touch-action:none;
      -webkit-tap-highlight-color:transparent;
      background:transparent; border:0;
    }
    #tk-touch-left { left:0; }
    #tk-touch-right { right:0; }
    /* No labels or pressed-state colours on either steering half. */
    #tk-touch-actions {
      position:absolute; left:50%; bottom:max(14px,env(safe-area-inset-bottom));
      transform:translateX(-50%); display:flex; align-items:flex-end;
      gap:clamp(12px,3vw,26px); pointer-events:none; z-index:1;
    }
    #tk-touch-actions .tk-action {
      pointer-events:auto; touch-action:none; -webkit-tap-highlight-color:transparent;
      width:clamp(65px,13vw,100px); height:clamp(65px,13vw,100px);
      border-radius:50%; border:2px solid rgba(255,255,255,.8);
      box-shadow:0 2px 7px rgba(0,0,0,.45);
      background:rgba(15,33,53,.85); color:white; font-weight:900;
      font-size:clamp(12px,2vw,18px); line-height:1.1;
      text-align:center; white-space:pre-line;
      padding:7px; box-sizing:border-box;
      -webkit-user-select:none; user-select:none;
    }
    #tk-touch-fire { background:rgba(169,52,29,.91); }
    #tk-touch-look { background:rgba(24,68,118,.91); }
    /* The action buttons stay the same colour while touched. */
  `;
  doc.head.append(style);
  const overlay = doc.createElement('div');
  overlay.id = 'tk-touch-controls';
  overlay.setAttribute('aria-label', 'Auto-drive. Hold left or right half to steer. Separate Fire and Look Back buttons.');
  const listeners = [];
  const listen = (target, type, cb, opts) => {
    target.addEventListener(type, cb, opts);
    listeners.push([target, type, cb, opts]);
  };

  const sendKey = (key, down) => {
    const code = key;
    const keyName = key === 'KeyE' ? 'e' : key === 'KeyC' ? 'c' : key;
    doc.dispatchEvent(new win.KeyboardEvent(down ? 'keydown' : 'keyup', {
      key: keyName, code, bubbles:true, cancelable:true,
    }));
    win.__turboKartTouchState = state.snapshot();
  };

  let fireTimer = null;
  const stopFire = () => {
    if (fireTimer !== null) {
      win.clearTimeout(fireTimer);
      fireTimer = null;
      sendKey(CONTROL_KEYS.fire, false);
    }
  };
  const fire = () => {
    if (!state.active || fireTimer !== null) return;
    // Desktop tests hold E for 120ms; a tap should generate a comparable pulse.
    sendKey(CONTROL_KEYS.fire, true);
    fireTimer = win.setTimeout(() => {
      fireTimer = null;
      sendKey(CONTROL_KEYS.fire, false);
    }, 145);
  };
  const state = new TouchDriveState(sendKey, fire);
  const sideFromX = x => x < win.innerWidth / 2 ? 'left' : 'right';

  // Full-screen invisible steering zones. Pressing or releasing them NEVER fires.
  for (const side of ['left', 'right']) {
    const zone = doc.createElement('div');
    zone.id = 'tk-touch-' + side;
    zone.className = 'tk-half';
    zone.setAttribute('aria-label', 'Hold to steer ' + side);
    overlay.append(zone);

    listen(zone, 'pointerdown', e => {
      e.preventDefault();
      zone.setPointerCapture?.(e.pointerId);
      state.press(e.pointerId, sideFromX(e.clientX));
    });
    listen(zone, 'pointermove', e => {
      if (!state.pointers.has(e.pointerId)) return;
      e.preventDefault();
      state.move(e.pointerId, sideFromX(e.clientX));
    });
    listen(zone, 'pointerup', e => {
      e.preventDefault();
      state.release(e.pointerId);
    });
    const cancelSteer = e => state.release(e.pointerId);
    listen(zone, 'pointercancel', cancelSteer);
    listen(zone, 'lostpointercapture', cancelSteer);
    listen(zone, 'contextmenu', e => e.preventDefault());
  }

  // These native buttons sit *above* the steering halves and do not
  // dispatch any steering events, including while a second finger steers.
  const actions = doc.createElement('div');
  actions.id = 'tk-touch-actions';

  const fireButton = doc.createElement('button');
  fireButton.type = 'button';
  fireButton.id = 'tk-touch-fire';
  fireButton.className = 'tk-action';
  fireButton.textContent = 'FIRE';
  fireButton.setAttribute('aria-label', 'Fire equipped weapon');
  listen(fireButton, 'pointerdown', e => {
    e.preventDefault();
    e.stopPropagation();
    state.fireWeapon();
  });
  listen(fireButton, 'click', e => {
    // Keyboard / accessibility activation: normal pointer click already
    // fired on pointerdown, so do not fire twice.
    if (e.detail === 0) state.fireWeapon();
    e.preventDefault();
    e.stopPropagation();
  });
  listen(fireButton, 'contextmenu', e => e.preventDefault());
  actions.append(fireButton);

  const lookButton = doc.createElement('button');
  lookButton.type = 'button';
  lookButton.id = 'tk-touch-look';
  lookButton.className = 'tk-action';
  lookButton.textContent = 'LOOK BACK';
  lookButton.setAttribute('aria-label', 'Hold to look backwards');
  listen(lookButton, 'pointerdown', e => {
    e.preventDefault();
    e.stopPropagation();
    lookButton.setPointerCapture?.(e.pointerId);
    state.pressLook(e.pointerId);
  });
  const releaseLook = e => {
    e.preventDefault();
    e.stopPropagation?.();
    state.releaseLook(e.pointerId);
  };
  listen(lookButton, 'pointerup', releaseLook);
  listen(lookButton, 'pointercancel', releaseLook);
  listen(lookButton, 'lostpointercapture', releaseLook);
  // Space/Enter should also work if the button is focused with a keyboard.
  listen(lookButton, 'keydown', e => {
    if (e.code !== 'Space' && e.code !== 'Enter') return;
    e.preventDefault();
    if (!e.repeat) state.pressLook('keyboard');
  });
  listen(lookButton, 'keyup', e => {
    if (e.code !== 'Space' && e.code !== 'Enter') return;
    e.preventDefault();
    state.releaseLook('keyboard');
  });
  listen(lookButton, 'blur', () => state.releaseLook('keyboard'));
  listen(lookButton, 'contextmenu', e => e.preventDefault());
  actions.append(lookButton);

  overlay.append(actions);
  doc.body.append(overlay);

  // The touch overlay is completely absent from all menus and results screens.
  const isRace = () => ['intro', 'countdown', 'race', 'racing'].includes(doc.body?.dataset.state);
  let focused = true;
  const refresh = () => {
    const enabled = focused && doc.visibilityState !== 'hidden' && isRace();
    if (state.active !== enabled) {
      state.setActive(enabled);
      if (!enabled) stopFire();
    }
    overlay.classList.toggle('tk-active', enabled);
  };
  const cancel = () => {
    focused = false;
    state.setActive(false);
    stopFire();
    overlay.classList.remove('tk-active');
  };
  listen(win, 'blur', cancel);
  listen(win, 'pagehide', cancel);
  listen(win, 'focus', () => { focused = true; refresh(); });
  listen(doc, 'visibilitychange', refresh);
  const timer = win.setInterval(refresh, 100);
  win.__turboKartTouchState = state.snapshot();
  refresh();

  const detach = () => {
    win.clearInterval(timer);
    cancel();
    for (const [target, type, cb, opts] of listeners) {
      target.removeEventListener(type, cb, opts);
    }
    overlay.remove();
    style.remove();
    delete win.__turboKartTouchDetach;
    delete win.__turboKartTouchState;
  };
  win.__turboKartTouchDetach = detach;
  return detach;
}

if (typeof document !== 'undefined') {
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', () => attachTurboKartTouchControls(), { once:true });
  } else {
    attachTurboKartTouchControls();
  }
}
