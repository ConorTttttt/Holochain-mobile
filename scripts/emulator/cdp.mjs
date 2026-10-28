// Development-only loopback CDP helper, used for emulator and desktop evidence.
import fs from 'node:fs';
export async function connect(url) {
  const endpoint = new URL(url);
  if (!['127.0.0.1', 'localhost'].includes(endpoint.hostname)) throw new Error('CDP must use loopback');
  const targets = await (await fetch(`${url}/json/list`)).json();
  const target = targets.find(t => t.type === 'page') || targets[0];
  if (!target) throw new Error('No debug target');
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  const pending = new Map(); let sequence = 0;
  socket.onclose = () => { for (const call of pending.values()) call.reject(new Error('CDP connection closed')); pending.clear(); };
  socket.onmessage = event => {
    const response = JSON.parse(event.data);
    const call = pending.get(response.id);
    if (call) { pending.delete(response.id); response.error ? call.reject(new Error(JSON.stringify(response.error))) : call.resolve(response.result); }
  };
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    if (socket.readyState !== WebSocket.OPEN) { reject(new Error('CDP connection is not open')); return; }
    const id = ++sequence;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, 30000);
    pending.set(id, { resolve: v => { clearTimeout(timer); resolve(v); }, reject: e => { clearTimeout(timer); reject(e); } });
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async expression => {
    const result = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  return { send, evaluate, close: () => socket.close() };
}
if (process.argv[1] && import.meta.url === new URL(process.argv[1], 'file:').href) {
  const client = await connect(process.argv[2]);
  try { console.log(JSON.stringify(await client.evaluate(fs.readFileSync(0, 'utf8')), null, 2)); }
  finally { client.close(); }
}
