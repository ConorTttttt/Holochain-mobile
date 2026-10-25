/*
 * Turbo Kart Rally — Android touch controls (single-player preview).
 *
 * This layer does not implement new driving physics. Its synthetic keyboard
 * events use the desktop game's existing InputController key mappings:
 * ArrowLeft/ArrowRight, ArrowUp, ArrowDown, Space, KeyE, Escape.
 *
 * Multi-touch held keys are reference-counted by pointer. No virtual key can
 * stay down after loss of focus, pointer cancel, page hide or disposal.
 */
(() => {
  'use strict';
  if (window.TurboKartTouch) {
    // onPageFinished or a reload may re-inject the script.
    window.TurboKartTouch.dispose();
  }

  const style = document.createElement('style');
  style.id = 'turbo-kart-touch-style';
  style.textContent = `
    #tk-touch-controls {
      position: fixed; inset: 0; z-index: 1000000;
      pointer-events: none; user-select: none; -webkit-user-select: none;
      -webkit-touch-callout: none; touch-action: none;
      font-family: system-ui, sans-serif;
    }
    #tk-touch-controls[hidden] { display: none !important; }
    #tk-touch-controls .tk-cluster {
      position: absolute; display: flex; align-items: flex-end; gap: 9px;
      pointer-events: none;
    }
    #tk-touch-controls .tk-steer {
      left: max(14px, env(safe-area-inset-left));
      bottom: max(16px, env(safe-area-inset-bottom));
    }
    #tk-touch-controls .tk-action {
      right: max(14px, env(safe-area-inset-right));
      bottom: max(16px, env(safe-area-inset-bottom));
    }
    #tk-touch-controls .tk-column {
      display: flex; flex-direction: column; gap: 8px;
      pointer-events: none;
    }
    #tk-touch-controls .tk-pause {
      position: absolute;
      top: max(14px, env(safe-area-inset-top));
      right: max(14px, env(safe-area-inset-right));
    }
    #tk-touch-controls button {
      position: relative;
      width: clamp(57px, 11.5vh, 94px); height: clamp(57px, 11.5vh, 94px);
      padding: 4px; margin: 0; min-width: 0;
      border-radius: 22px; border: 3px solid rgba(255,255,255,.82);
      background: linear-gradient(165deg, rgba(59,77,177,.88), rgba(14,21,66,.9));
      color: white; font: 900 clamp(13px,2.6vh,20px)/1 system-ui,sans-serif;
      letter-spacing: .3px; text-align: center;
      text-shadow: 0 2px 2px rgba(0,0,0,.7);
      box-shadow: 0 4px 0 rgba(0,0,0,.48), 0 6px 13px rgba(0,0,0,.4);
      pointer-events: auto; touch-action: none; overscroll-behavior: contain;
      -webkit-tap-highlight-color: transparent;
      outline-offset: 3px;
    }
    #tk-touch-controls button[data-action="left"],
    #tk-touch-controls button[data-action="right"] {
      font-size: clamp(28px, 6vh, 50px);
    }
    #tk-touch-controls button[data-action="accelerate"] {
      background: linear-gradient(165deg, rgba(30,174,91,.93), rgba(4,94,49,.94));
    }
    #tk-touch-controls button[data-action="brake"] {
      background: linear-gradient(165deg, rgba(255,105,57,.91), rgba(168,40,29,.94));
    }
    #tk-touch-controls button[data-action="item"] {
      background: linear-gradient(165deg, rgba(244,189,35,.96), rgba(158,90,15,.95));
    }
    #tk-touch-controls button[data-action="pause"] {
      width: 53px; height: 46px; border-radius: 14px;
      font-size: 15px; background: rgba(13,20,59,.85);
    }
    #tk-touch-controls button.tk-held {
      transform: translateY(3px); box-shadow: 0 1px 0 rgba(0,0,0,.4);
      filter: brightness(1.35);
    }
    @media (max-height: 430px) {
      #tk-touch-controls button {
        width: clamp(50px, 12vh, 68px);
        height: clamp(50px, 12vh, 68px);
        border-radius: 17px; font-size: 13px;
      }
      #tk-touch-controls .tk-cluster { gap: 7px; }
      #tk-touch-controls .tk-column { gap: 7px; }
    }
  `;
  document.head.appendChild(style);

  const root = document.createElement('div');
  root.id = 'tk-touch-controls';
  root.setAttribute('aria-label', 'Turbo Kart touch game controls');
  root.hidden = true;
  const held = new Map(); // pointerId -> { button, code }
  const activeCodes = new Map(); // code -> number of active pointers
  const buttons = new Map();
  const codeOf = {
    left: 'ArrowLeft', right: 'ArrowRight',
    accelerate: 'ArrowUp', brake: 'ArrowDown',
    drift: 'Space', item: 'KeyE', pause: 'Escape',
  };
  const keyOf = {
    ArrowLeft: 'ArrowLeft', ArrowRight: 'ArrowRight',
    ArrowUp: 'ArrowUp', ArrowDown: 'ArrowDown',
    Space: ' ', KeyE: 'e', Escape: 'Escape',
  };

  function emit(code, kind) {
    window.dispatchEvent(new KeyboardEvent(kind, {
      bubbles: true, cancelable: true, code, key: keyOf[code],
    }));
  }

  function release(pointerId) {
    const state = held.get(pointerId);
    if (!state) return;
    held.delete(pointerId);
    const count = (activeCodes.get(state.code) || 1) - 1;
    if (count <= 0) {
      activeCodes.delete(state.code);
      emit(state.code, 'keyup');
      state.button.classList.remove('tk-held');
    } else {
      activeCodes.set(state.code, count);
    }
  }

  function press(e, action, button) {
    if (e.type !== 'pointerdown' || !e.isPrimary && e.pointerType === 'mouse') return;
    if (held.has(e.pointerId)) return;
    e.preventDefault();
    e.stopPropagation();
    const code = codeOf[action];
    const count = activeCodes.get(code) || 0;
    held.set(e.pointerId, { code, button });
    activeCodes.set(code, count + 1);
    if (count === 0) {
      emit(code, 'keydown');
      button.classList.add('tk-held');
    }
    // Captured pointers still receive their release when dragged off a button.
    try { button.setPointerCapture?.(e.pointerId); } catch { /* cancelled pointer */ }
  }

  function makeButton(action, label, ariaLabel) {
    const button = document.createElement('button');
    button.type = 'button';
    button.dataset.action = action;
    button.textContent = label;
    button.setAttribute('aria-label', ariaLabel);
    button.addEventListener('pointerdown', (e) => press(e, action, button));
    for (const eventName of ['pointerup', 'pointercancel', 'lostpointercapture']) {
      button.addEventListener(eventName, (e) => {
        e.preventDefault();
        e.stopPropagation();
        release(e.pointerId);
      });
    }
    button.addEventListener('contextmenu', (e) => e.preventDefault());
    buttons.set(action, button);
    return button;
  }

  const steer = document.createElement('div');
  steer.className = 'tk-cluster tk-steer';
  steer.append(
    makeButton('left', '◀', 'Steer left'),
    makeButton('right', '▶', 'Steer right'),
  );

  const right = document.createElement('div');
  right.className = 'tk-cluster tk-action';
  const mini = document.createElement('div');
  mini.className = 'tk-column';
  mini.append(
    makeButton('item', 'ITEM', 'Use item'),
    makeButton('drift', 'DRIFT', 'Hop and drift'),
  );
  right.append(
    makeButton('brake', 'BRAKE', 'Brake and reverse'),
    mini,
    makeButton('accelerate', 'GO', 'Accelerate'),
  );
  const pause = document.createElement('div');
  pause.className = 'tk-pause';
  pause.append(makeButton('pause', 'Ⅱ', 'Pause'));

  root.append(steer, right, pause);
  document.body.appendChild(root);

  function releaseAll() {
    for (const pointerId of [...held.keys()]) release(pointerId);
  }

  // Existing Turbo Kart exposes window.__game.state and body[data-state].
  // Show driving buttons only during intro, countdown and actual gameplay;
  // normal title/character-select DOM remains directly touchable.
  const visibleStates = new Set(['intro', 'countdown', 'racing']);
  function updateVisibility() {
    const state = window.__game?.state || document.body?.dataset?.state;
    const shouldShow = visibleStates.has(state);
    if (!shouldShow && !root.hidden) releaseAll();
    root.hidden = !shouldShow;
  }
  updateVisibility();
  const ticker = window.setInterval(updateVisibility, 120);
  const onBlur = () => releaseAll();
  const onVisibility = () => {
    if (document.hidden) releaseAll();
    updateVisibility();
  };
  window.addEventListener('blur', onBlur);
  window.addEventListener('pagehide', onBlur);
  document.addEventListener('visibilitychange', onVisibility);

  window.TurboKartTouch = {
    releaseAll,
    // For test diagnostics, not used as a game authority.
    get heldActions() {
      return [...activeCodes.keys()];
    },
    dispose() {
      releaseAll();
      window.clearInterval(ticker);
      window.removeEventListener('blur', onBlur);
      window.removeEventListener('pagehide', onBlur);
      document.removeEventListener('visibilitychange', onVisibility);
      root.remove();
      style.remove();
      delete window.TurboKartTouch;
    },
  };
})();
