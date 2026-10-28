// Independently check the original desktop's Holochain view and rendered room.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { connect } from './cdp.mjs';
const [label, lobbyId, androidAgent, expectedCount] = process.argv.slice(2);
if (!/^[a-z0-9-]+$/.test(label || '') || !lobbyId || !androidAgent || !['0','1'].includes(expectedCount)) throw new Error('label lobbyId androidAgent expectedCount required');
const client = await connect('http://127.0.0.1:9333');
const output = 'docs/evidence/combined';
try {
  const code = `(async()=>{const state=await kartNet.lobbyGet(${JSON.stringify(lobbyId)});__game.menu.showMultiplayer();__game.menu.multiplayer.room(state);await new Promise(r=>setTimeout(r,450));return {state,visibleText:document.body.innerText};})()`;
  const result = await client.evaluate(`(async()=>{const E=process.mainModule.require('electron');const w=E.BrowserWindow.getAllWindows().find(w=>w.webContents.getURL().startsWith('webhapp://'));return {view:await w.webContents.executeJavaScript(${JSON.stringify(code)},true),image:(await w.webContents.capturePage()).toPNG().toString('base64')};})()`);
  assert.equal(result.view.state.members.filter(m=>m.agent===androidAgent).length, Number(expectedCount));
  if(expectedCount==='1') assert.ok(result.view.visibleText.includes('Zippy'));
  fs.writeFileSync(path.join(output,`desktop-${label}.json`),JSON.stringify(result.view,null,2)+'\n');
  fs.writeFileSync(path.join(output,`desktop-${label}.png`),Buffer.from(result.image,'base64'));
  console.log(`PASS original desktop observes Android membership=${expectedCount}`);
} finally { client.close(); }
