// Turbo Kart mobile racing overlay. No dependencies, no changes to desktop controls.
// Import from index.html as a module, or from tests (no DOM at import time).
export const CONTROL_KEYS = Object.freeze({ left: 'ArrowLeft', right: 'ArrowRight', go: 'ArrowUp' });

// One finger can steer while another holds GO. Independent pointer IDs matter:
// mobile browsers do not turn multi-touch into reliable mouse events.
export class TouchKeyState {
  constructor(onKeyChange = () => {}) {
    this.onKeyChange = onKeyChange;
    this.pointers = new Map();
    this.activeKeys = new Set();
  }
  press(pointerId, action) {
    if (!Object.hasOwn(CONTROL_KEYS, action)) throw new Error('Unknown touch action');
    this.pointers.set(pointerId, action);
    this.sync();
  }
  release(pointerId) {
    this.pointers.delete(pointerId);
    this.sync();
  }
  releaseAll() {
    this.pointers.clear();
    this.sync();
  }
  snapshot() {
    const actions = new Set(this.pointers.values());
    // Two opposing steer touches cancel out; acceleration remains independent.
    const left = actions.has('left') && !actions.has('right');
    const right = actions.has('right') && !actions.has('left');
    return { left, right, go: actions.has('go') };
  }
  sync() {
    const s = this.snapshot();
    const next = new Set(Object.keys(CONTROL_KEYS)
      .filter(action => s[action]).map(action => CONTROL_KEYS[action]));
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
  // A single overlay per game page; restarting the race reuses it.
  if (win.__turboKartTouchDetach) return win.__turboKartTouchDetach;

  const style = doc.createElement('style');
  style.textContent = `
    #tk-touch-controls { position:fixed; inset:0; z-index:9990; display:none; pointer-events:none;
      font-family:system-ui,sans-serif; user-select:none; -webkit-user-select:none; }
    #tk-touch-controls.tk-active { display:block; }
    #tk-touch-controls .tk-zone { position:absolute; display:flex; align-items:flex-end;
      justify-content:center; pointer-events:auto; touch-action:none; -webkit-tap-highlight-color:transparent;
      border:2px solid rgba(255,255,255,.4); box-sizing:border-box; border-radius:24px;
      color:white; background:rgba(14,26,45,.29); font-size:clamp(22px,6vw,52px);
      font-weight:900; padding-bottom:max(10px, env(safe-area-inset-bottom));
      text-shadow:0 2px 5px #0008; }
    #tk-touch-controls .tk-zone.tk-held { background:rgba(35,136,212,.60); border-color:white; }
    #tk-touch-left { left:max(10px,env(safe-area-inset-left)); bottom:max(12px,env(safe-area-inset-bottom)); width:29vw; height:47vh; }
    #tk-touch-right { right:max(10px,env(safe-area-inset-right)); bottom:max(12px,env(safe-area-inset-bottom)); width:29vw; height:47vh; }
    #tk-touch-go { left:35%; width:30%; height:29vh; bottom:max(12px,env(safe-area-inset-bottom));
      background:rgba(13,105,57,.52) !important; }
    #tk-touch-go.tk-held { background:rgba(29,190,96,.77) !important; }
    @media (orientation:portrait) { #tk-touch-controls .tk-zone { font-size:22px; border-radius:12px; }
      #tk-touch-left,#tk-touch-right { height:30vh; width:30vw; }
      #tk-touch-go { height:21vh; }
    }
    @media (prefers-reduced-motion:reduce) { #tk-touch-controls * { animation:none !important; } }
  `;
  doc.head.append(style);
  const overlay = doc.createElement('div');
  overlay.id = 'tk-touch-controls';
  overlay.setAttribute('aria-label', 'Turbo Kart touch driving controls');
  const spec = [
    { id:'tk-touch-left', action:'left', text:'◀', label:'Steer left' },
    { id:'tk-touch-go', action:'go', text:'GO ▲', label:'Hold to accelerate' },
    { id:'tk-touch-right', action:'right', text:'▶', label:'Steer right' }
  ];
  const state = new TouchKeyState((code, pressed) => {
    const event = new win.KeyboardEvent(pressed ? 'keydown' : 'keyup', {
      key: code, code, bubbles: true, cancelable: true
    });
    doc.dispatchEvent(event);
    win.__turboKartTouchState = state.snapshot(); // alternate direct input path if needed
  });
  const zones = new Map();
  const listeners = [];
  const listen = (node, type, callback, opts) => {
    node.addEventListener(type, callback, opts);
    listeners.push([node, type, callback, opts]);
  };
  const refreshHeld = () => {
    const held = new Set(state.pointers.values());
    for (const [action, el] of zones) el.classList.toggle('tk-held', held.has(action));
  };
  for (const { id, action, text, label } of spec) {
    const el = doc.createElement('div');
    el.id = id;
    el.className = 'tk-zone';
    el.textContent = text;
    el.setAttribute('role', 'button');
    el.setAttribute('aria-label', label);
    el.setAttribute('aria-keyshortcuts', CONTROL_KEYS[action]);
    zones.set(action, el);
    overlay.append(el);
    listen(el, 'pointerdown', e => {
      if (!e.isPrimary && e.pointerType === 'mouse') return;
      e.preventDefault();
      el.setPointerCapture?.(e.pointerId);
      state.press(e.pointerId, action);
      refreshHeld();
    });
    const release = e => { e.preventDefault(); state.release(e.pointerId); refreshHeld(); };
    listen(el, 'pointerup', release);
    listen(el, 'pointercancel', release);
    listen(el, 'lostpointercapture', release);
    listen(el, 'contextmenu', e => e.preventDefault());
  }
  doc.body.append(overlay);
  // Menus are normal click targets; touch controls must not cover them.
  // The game's body.dataset.state is 'countdown' or 'race' during a race.
  const isRace = () => ['countdown','race','racing','intro'].includes(doc.body?.dataset.state);
  const refresh = () => {
    const visible = isRace() && doc.visibilityState !== 'hidden';
    if (!visible && state.pointers.size) { state.releaseAll(); refreshHeld(); }
    overlay.classList.toggle('tk-active', visible);
  };
  refresh();
  const timer = win.setInterval(refresh, 130);
  const cancelAll = () => { state.releaseAll(); refreshHeld(); };
  listen(win, 'blur', cancelAll);
  listen(win, 'pagehide', cancelAll);
  listen(doc, 'visibilitychange', refresh);
  win.__turboKartTouchState = state.snapshot();
  const detach = () => {
    win.clearInterval(timer);
    cancelAll();
    for (const [node, type, callback, opts] of listeners) node.removeEventListener(type, callback, opts);
    overlay.remove(); style.remove();
    delete win.__turboKartTouchDetach;
    delete win.__turboKartTouchState;
  };
  win.__turboKartTouchDetach = detach;
  return detach;
}

if (typeof document !== 'undefined') {
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', () => attachTurboKartTouchControls(), { once:true });
  else attachTurboKartTouchControls();
}
