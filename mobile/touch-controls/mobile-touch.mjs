// Turbo Kart landscape touchscreen controls.
//
// Auto-accelerate during a race. Holding the left/right half steers.
// A brief stationary tap (on either half) fires the equipped item with E.
// Browser-native pointer IDs allow multi-touch, and the game itself continues
// to own physics, weapon selection, item cooldowns, and race state.
export const CONTROL_KEYS = Object.freeze({
  left: 'ArrowLeft',
  right: 'ArrowRight',
  accelerate: 'ArrowUp',
  fire: 'KeyE',
});
export const TAP_MAX_MS = 230;
export const TAP_MAX_DISTANCE_PX = 22;

export class TouchDriveState {
  constructor(onKeyChange = () => {}, onFire = () => {}) {
    this.onKeyChange = onKeyChange;
    this.onFire = onFire;
    this.active = false;
    this.pointers = new Map();
    this.activeKeys = new Set();
  }

  setActive(active) {
    const next = Boolean(active);
    if (!next) this.pointers.clear();
    this.active = next;
    this.sync();
  }

  press(pointerId, side, x, y, at = 0) {
    if (!this.active) return;
    if (side !== 'left' && side !== 'right') throw new Error('Unknown steering side');
    // Reusing an existing pointer should never count as a new tap.
    if (this.pointers.has(pointerId)) return;
    this.pointers.set(pointerId, {
      side, x, y, startedAt: at, moved: false,
    });
    this.sync();
  }

  move(pointerId, x, y, side) {
    const pointer = this.pointers.get(pointerId);
    if (!pointer) return;
    if (side !== 'left' && side !== 'right') throw new Error('Unknown steering side');
    const dx = x - pointer.x;
    const dy = y - pointer.y;
    if (dx * dx + dy * dy > TAP_MAX_DISTANCE_PX * TAP_MAX_DISTANCE_PX) {
      pointer.moved = true;
    }
    if (pointer.side !== side) {
      pointer.side = side;
      pointer.moved = true; // crossing halves is a steer, not a weapon tap
      this.sync();
    }
  }

  release(pointerId, at = 0, allowTap = true) {
    const pointer = this.pointers.get(pointerId);
    if (!pointer) return;
    this.pointers.delete(pointerId);
    this.sync();
    const duration = at - pointer.startedAt;
    if (allowTap && this.active && !pointer.moved &&
        duration >= 0 && duration <= TAP_MAX_MS) {
      this.onFire();
    }
  }

  releaseAll() {
    if (this.pointers.size) this.pointers.clear();
    this.sync();
  }

  snapshot() {
    const sides = [...this.pointers.values()].map(p => p.side);
    return {
      left: this.active && sides.includes('left') && !sides.includes('right'),
      right: this.active && sides.includes('right') && !sides.includes('left'),
      accelerate: this.active,
    };
  }

  sync() {
    const snapshot = this.snapshot();
    const next = new Set();
    if (snapshot.left) next.add(CONTROL_KEYS.left);
    if (snapshot.right) next.add(CONTROL_KEYS.right);
    if (snapshot.accelerate) next.add(CONTROL_KEYS.accelerate);
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
    #tk-touch-controls .tk-half.tk-held { background:rgba(25,114,196,.09); }
    #tk-touch-controls .tk-hint {
      position:absolute; bottom:max(16px,env(safe-area-inset-bottom));
      font-size:clamp(13px,2.5vw,21px); font-weight:800;
      color:white; background:rgba(0,0,0,.43);
      border:1px solid rgba(255,255,255,.45);
      border-radius:999px; padding:6px 14px;
      text-shadow:0 1px 3px black;
      pointer-events:none;
    }
    #tk-touch-left .tk-hint { left:max(12px,env(safe-area-inset-left)); }
    #tk-touch-right .tk-hint { right:max(12px,env(safe-area-inset-right)); }
    #tk-touch-controls .tk-status {
      position:absolute; bottom:max(16px,env(safe-area-inset-bottom));
      left:50%; transform:translateX(-50%);
      font-size:clamp(10px,1.7vw,16px); padding:5px 10px;
      border-radius:10px; pointer-events:none;
      color:#fff; background:rgba(0,0,0,.39); white-space:nowrap;
    }
  `;
  doc.head.append(style);
  const overlay = doc.createElement('div');
  overlay.id = 'tk-touch-controls';
  overlay.setAttribute('aria-label', 'Auto-drive: hold left or right half to steer, tap either side to fire');
  const zones = new Map();
  const listeners = [];
  const listen = (target, type, cb, opts) => {
    target.addEventListener(type, cb, opts);
    listeners.push([target, type, cb, opts]);
  };

  const sendKey = (key, down) => {
    const code = key === 'KeyE' ? 'KeyE' : key;
    const keyName = key === 'KeyE' ? 'e' : key;
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
  const refreshHeld = () => {
    const sides = new Set([...state.pointers.values()].map(p => p.side));
    for (const [side, node] of zones) node.classList.toggle('tk-held', sides.has(side));
  };

  for (const { side, title } of [
    {side:'left', title:'◀ HOLD LEFT · TAP TO FIRE'},
    {side:'right', title:'HOLD RIGHT ▶ · TAP TO FIRE'},
  ]) {
    const node = doc.createElement('div');
    node.id = 'tk-touch-' + side;
    node.className = 'tk-half';
    node.setAttribute('aria-label', title);
    const hint = doc.createElement('div');
    hint.className = 'tk-hint';
    hint.textContent = title;
    node.append(hint);
    zones.set(side, node);
    overlay.append(node);

    listen(node, 'pointerdown', e => {
      e.preventDefault();
      node.setPointerCapture?.(e.pointerId);
      state.press(e.pointerId, sideFromX(e.clientX), e.clientX, e.clientY, e.timeStamp);
      refreshHeld();
    });
    listen(node, 'pointermove', e => {
      if (!state.pointers.has(e.pointerId)) return;
      e.preventDefault();
      state.move(e.pointerId, e.clientX, e.clientY, sideFromX(e.clientX));
      refreshHeld();
    });
    listen(node, 'pointerup', e => {
      e.preventDefault();
      state.release(e.pointerId, e.timeStamp);
      refreshHeld();
    });
    const cancel = e => {
      state.release(e.pointerId, e.timeStamp, false);
      refreshHeld();
    };
    listen(node, 'pointercancel', cancel);
    listen(node, 'lostpointercapture', cancel);
    listen(node, 'contextmenu', e => e.preventDefault());
  }
  const status = doc.createElement('div');
  status.className = 'tk-status';
  status.textContent = 'AUTO GO';
  overlay.append(status);
  doc.body.append(overlay);

  // The touch overlay is completely absent from all menus and results screens.
  const isRace = () => ['intro', 'countdown', 'race', 'racing'].includes(doc.body?.dataset.state);
  let focused = true;
  const refresh = () => {
    const enabled = focused && doc.visibilityState !== 'hidden' && isRace();
    if (state.active !== enabled) {
      state.setActive(enabled);
      if (!enabled) stopFire();
      refreshHeld();
    }
    overlay.classList.toggle('tk-active', enabled);
  };
  const cancel = () => {
    focused = false;
    state.setActive(false);
    stopFire();
    refreshHeld();
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
