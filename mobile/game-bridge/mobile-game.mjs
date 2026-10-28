import { GameBridge } from './game-bridge-core.mjs';
import { kartSnapshot, SnapshotBuffer } from './src/netplay.js';
import { CHARACTERS } from './src/config.js';
import { enterOnlineGame } from './online-game.mjs';

let bridge = null;
let focused = true;
window.addEventListener('blur', () => { focused = false; });
window.addEventListener('focus', () => { focused = true; });
window.addEventListener('message', async event => {
  // Native postWebMessage has no web sender origin. A document-scoped
  // random capability authenticates the transfer; Android also pins targetOrigin.
  if (location.origin !== 'https://appassets.androidplatform.net' || window.top !== window ||
      !window.__turboKartGamePortToken ||
      ![`turbo-kart-game-port:${window.__turboKartGamePortToken}`,`turbo-kart-online-port:${window.__turboKartGamePortToken}`].includes(event.data) || event.ports.length !== 1) return;
  const online=event.data.startsWith('turbo-kart-online-port:');
  delete window.__turboKartGamePortToken;
  const port = event.ports[0];
  // Page completion can precede the game's asynchronous module boot.
  for (let i = 0; i < 200 && !window.__game?.world; i++) await new Promise(r => setTimeout(r, 50));
  if (!window.__game?.world) { port.close(); return; }
  if(online){
    try{window.__turboKartOnlineSession=await enterOnlineGame(port,window.__game);}
    catch(error){console.error('Android online entry:',error);window.__turboKartOnline={error:String(error)};}
    return;
  }
  bridge?.close();
  bridge = new GameBridge({ port, game: window.__game, snapshot: kartSnapshot, Buffer: SnapshotBuffer, characters: CHARACTERS });
  window.__turboKartGameBridge = bridge.stats;
  const label = document.createElement('div');
  label.textContent = 'DEVELOPMENT MULTIPLAYER · timing and scoring unavailable';
  Object.assign(label.style, { position:'fixed', top:'0', left:'0', zIndex:10000, background:'#000c', color:'white', font:'12px sans-serif', pointerEvents:'none' });
  document.body.append(label);
});
let last = performance.now();
let menuHint = null;
function frame(now) {
  const hint = document.querySelector('.desktop-required');
  if (hint && hint !== menuHint) {
    hint.textContent = 'Multiplayer: use Android Back to return to the app menu.';
    Object.assign(hint.style, { position:'fixed', left:'0', bottom:'12vh', width:'100%', margin:'0', fontSize:'11px', lineHeight:'1.2' });
    menuHint = hint;
  }
  bridge?.update(Math.min(0.05, (now - last) / 1000), focused && document.visibilityState !== 'hidden');
  last = now;
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
window.addEventListener('pagehide', () => bridge?.close());
