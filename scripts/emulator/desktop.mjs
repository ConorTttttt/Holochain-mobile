// Evaluate test code in the original desktop's sandboxed renderer API.
import fs from 'node:fs';
import { connect } from './cdp.mjs';
const code = fs.readFileSync(0, 'utf8');
const client = await connect(process.argv[2] || 'http://127.0.0.1:9333');
try {
  const value = await client.evaluate(`(async()=>{const E=process.mainModule.require('electron');const w=E.BrowserWindow.getAllWindows().find(w=>w.webContents.getURL().startsWith('webhapp://'));if(!w)throw new Error('Desktop game window absent');return await w.webContents.executeJavaScript(${JSON.stringify(code)},true);})()`);
  console.log(JSON.stringify(value, null, 2));
} finally { client.close(); }
