#!/usr/bin/env node
/**
 * WebSocket client for testing h2hc-linker DHT queries.
 *
 * Connects to the gateway WebSocket, authenticates, and registers a fake agent
 * so the gateway joins the kitsune2 space and discovers conductor peers.
 *
 * Usage:
 *   node test-ws-client.mjs <dna_hash> [gateway_url]
 *
 * The script stays connected until killed (Ctrl+C) to keep the agent registered.
 */

import { WebSocket } from 'ws';
import { randomBytes } from 'crypto';

const DNA_HASH = process.argv[2];
const GATEWAY_URL = process.argv[3] || 'ws://localhost:8000/ws';

if (!DNA_HASH) {
  console.error('Usage: node test-ws-client.mjs <dna_hash> [gateway_url]');
  console.error('  dna_hash: base64-encoded DNA hash from conductor (e.g., uhC0k...)');
  process.exit(1);
}

// Generate a fake agent pubkey (39 bytes: 3 prefix + 32 random + 4 location)
// AgentPubKey prefix: 0x84, 0x20, 0x24
function generateFakeAgentPubKey() {
  const raw32 = randomBytes(32);
  // Use holo_hash format: 3-byte prefix + 32-byte hash + 4-byte DHT location
  // For AgentPubKey, prefix is [132, 32, 36] = [0x84, 0x20, 0x24]
  const prefix = Buffer.from([0x84, 0x20, 0x24]);
  // DHT location is last 4 bytes of Blake2b hash, but we'll just use zeros
  // (the gateway doesn't validate the location bytes)
  const location = Buffer.from([0x00, 0x00, 0x00, 0x00]);
  const full39 = Buffer.concat([prefix, raw32, location]);
  // Base64 encode for WebSocket message
  return full39.toString('base64');
}

const AGENT_PUBKEY = generateFakeAgentPubKey();

console.log(`Connecting to gateway: ${GATEWAY_URL}`);
console.log(`DNA hash: ${DNA_HASH}`);
console.log(`Agent pubkey: ${AGENT_PUBKEY.substring(0, 20)}...`);

const ws = new WebSocket(GATEWAY_URL);

ws.on('open', () => {
  console.log('WebSocket connected');

  // Step 1: Authenticate
  const authMsg = JSON.stringify({ type: 'auth', session_token: '' });
  console.log(`Sending: ${authMsg}`);
  ws.send(authMsg);
});

ws.on('message', (data) => {
  const msg = JSON.parse(data.toString());
  console.log(`Received: ${JSON.stringify(msg)}`);

  if (msg.type === 'auth_ok') {
    // Step 2: Register agent for the DNA
    const registerMsg = JSON.stringify({
      type: 'register',
      dna_hash: DNA_HASH,
      agent_pubkey: AGENT_PUBKEY,
    });
    console.log(`Sending: ${registerMsg.substring(0, 80)}...`);
    ws.send(registerMsg);
  }

  if (msg.type === 'registered') {
    console.log('Agent registered successfully.');
    console.log('Keeping connection alive for DHT queries...');
    console.log('Press Ctrl+C to disconnect.');
  }

  if (msg.type === 'sign_request') {
    // Respond with a fake signature (64 zero bytes)
    const fakeSignature = Buffer.alloc(64).toString('base64');
    const signResponse = JSON.stringify({
      type: 'sign_response',
      request_id: msg.request_id,
      signature: fakeSignature,
    });
    console.log(`Sending fake sign response for request ${msg.request_id}`);
    ws.send(signResponse);
  }
});

ws.on('error', (err) => {
  console.error(`WebSocket error: ${err.message}`);
  process.exit(1);
});

ws.on('close', (code, reason) => {
  console.log(`WebSocket closed: code=${code} reason=${reason}`);
  process.exit(0);
});

// Keep alive with periodic pings
setInterval(() => {
  if (ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify({ type: 'ping' }));
  }
}, 30000);

// Graceful shutdown
process.on('SIGINT', () => {
  console.log('\nDisconnecting...');
  ws.close();
  process.exit(0);
});
