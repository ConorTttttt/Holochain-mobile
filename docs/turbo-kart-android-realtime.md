# Turbo Kart Android realtime phase — transport compatibility gate

**Status (25 October 2026):** **Gate 1 passed on the real Galaxy S23**.
The isolated Android arm64 executable compiled, passed 16 KiB ELF alignment,
matched the desktop's original WP3 42-byte golden test, and successfully
bound the original Iroh/MoQ transport. The user reported JSON events
`kart_wire_pass`, `iroh_endpoint_bound`, and `complete/ok:true`.
The probe returned direct IP addresses and an ephemeral endpoint ID;
this is **not** a race session or signed presence publication.

**Gate 2a passed on the Galaxy S23**: the user reports that the
updated APK built and installed, and after force-stop/relaunch it restored
the exact same **persistent Holochain agent and Iroh EndpointId**. Earlier
Holochain lobby joins/leaves and the 16 KiB alignment checks remain the
verified baseline.

**Gate 2b signed presence is now implemented in source, not yet rebuilt or
tested on the Galaxy S23.** It generates the exact original desktop
`SignedPresence` with a 120-second expiry, submits it through the
persisted Holochain agent and original `moq_directory` zome, and reads back
the action. A verified, admitted Iroh/MoQ peer session remains future work.

## What the desktop game already does

The desktop game's existing production pipeline is:

1. Both players are admitted to the same public Holochain lobby.
2. **Each player's own long-lived Iroh EndpointId** must be advertised in
   `gaming_platform.happ` as a **signed endpoint binding**, authored by that
   player's Holochain agent. An unsigned endpoint address is not enough.
3. The host calls the original `start_race` zome and locks immutable
   `RaceAdmission`, including the Holochain agents, their endpoint keys,
   characters, host and race ID.
4. `IrohMeshTransport` authenticates the peer's Ed25519 endpoint key over
   Iroh/QUIC and uses MoQ draft-16 for `race/<race>/<agent>/state`, plus
   existing event and host tracks. The race roster validates that the
   authenticated endpoint belongs to the admitted Holochain agent.
5. Game-local physics publishes a strict 42-byte `KartState` packet every
   50 ms (20 Hz), with snapshots interpolated for remote karts. **No
   Holochain zome calls occur in the per-frame data path.**

See the original desktop repository's
`apps/turbo-kart-desktop/E61-RACE-START.md`,
`E62-REALTIME-SESSIONS.md`, `E63-KART-SYNC.md` and
`crates/turbo-kart-client/src/race.rs`. These are the sources of truth;
do not fork the wire format or bypass authenticated race admission.

## Gate 1 — build and run *original desktop crates* on Android

`scripts/build-turbo-kart-realtime-probe.sh` builds a **separate
test-only Android arm64 executable**. Its Cargo manifest is generated
under the ignored `target/` directory, referencing the existing local
desktop game's:

- `crates/mesh-iroh`: actual `IrohMeshTransport` with Iroh 1.2.0 and
  MoQ draft-16.
- `crates/kart-wire`: original 42-byte WP3 `KartState` codec.

This probe checks the original desktop golden vector, round-trip decoding,
and rejection of truncated or unknown-version packets, then binds an
**ephemeral direct-only Iroh/MoQ endpoint** on the phone and prints its
EndpointId and available local addresses.

It does **not** publish signed Holochain presence, connect to the desktop,
admit anyone to a race, start a MoQ session, send kart packets, install an
APK, or persist a player key. It cannot be used as a game client.

Run on the existing Linux workstation (no benchmarks and no change to the
existing APK):

```sh
cd /mnt/holochain/TurboMobile/Holochain-mobile
git fetch origin feat/turbo-kart-integration
git switch feat/turbo-kart-integration
git merge --ff-only FETCH_HEAD

# Uses desktop game source at the known workstation path by default.
# Optionally set TURBO_KART_REPO=/path/to/turbo-kart.
bash scripts/build-turbo-kart-realtime-probe.sh
```

The first build of the Iroh/MoQ dependency tree can take time and requires
network downloads of Rust dependencies that are not yet cached. Cargo builds
only this isolated probe and its dependencies. Its output is:

```text
target/turbo-kart-realtime-probe-build/aarch64-linux-android/release/turbo-kart-realtime-probe
```

The script uses the existing Android NDK r27c compiler and verifies **16 KiB
ELF LOAD alignment**, as the APK build does. It does not alter either
source repository's Cargo.toml and does not touch the existing Holochain
smoke-test binary.

If it cross-compiles, push **only this disposable canary** to the phone:

```sh
export PATH="$HOME/Android/Sdk/platform-tools:$PATH"
adb devices -l
PROBE=target/turbo-kart-realtime-probe-build/aarch64-linux-android/release/turbo-kart-realtime-probe
adb push "$PROBE" /data/local/tmp/turbo-kart-realtime-probe
adb shell chmod 755 /data/local/tmp/turbo-kart-realtime-probe
adb shell /data/local/tmp/turbo-kart-realtime-probe
```

The expected JSON event names are:

```text
starting
kart_wire_pass
iroh_endpoint_bound
complete
```

Acceptance criteria:

- Rust/NDK Android arm64 cross-build and ELF alignment pass.
- `kart_wire_pass` reports a 42-byte packet and exact original desktop
  golden bytes.
- `iroh_endpoint_bound` reports a real Iroh EndpointId and local IP
  address, not just a `std::net::UdpSocket` binding.
- `complete` contains `ok:true`. A single endpoint that binds does NOT
  prove Iroh peer connectivity or MoQ track transport.

A failure at any gate should be captured without changing the APK, DNA
hashes, Holochain keystore, or desktop game.

## Gate 2 design — authenticated Android endpoint and race admission

After Gate 1 succeeds, add the Iroh/MoQ transport behind the **existing
Android JNI host**, not as a competing conductor or separate app process.

- Generate and persist a **separate** Ed25519 endpoint secret protected by
  the Android Keystore, with its own cryptographic domain and recovery
  policy. Keep raw key bytes away from the Activity, logs and renderer.
- Bind a single long-lived Iroh endpoint inside the Android application,
  with a stable identity across process restarts. Observe Android's
  network and foreground/background lifecycle, instead of assuming a
  constant Wi-Fi address or socket.
- Build the existing `SignedPresence` shape, including
  `iroh_binding_payload(endpoint_id, holochain_agent)` signed with the
  **Iroh endpoint's private key**. Use the Android conductor's own agent
  to call `moq_directory.publish_signed_presence` in the **original
  gaming directory hApp**, and refresh it before its TTL expires.
  Do not invent signatures or use the app's capability signer in place
  of the Holochain agent.
- Read back the directory's endpoint binding and verify identity,
  expiry and owner before allowing race start.
- Call the original Turbo Kart `get_race` and verify the persisted
  locked admission. Reuse `RaceAdmission::from_locked`; reject missing
  local membership, mismatched endpoint keys, duplicate endpoints or
  invalid/expired start binding.
- Build a real authenticated MoQ connection to the desktop using that
  frozen roster. Test session connect/stop/reconnect **without any race
  physics or game rendering**.

## Gate 2a — persistent Iroh EndpointId in the Android APK (source ready)

Files:

- `prototypes/turbo-kart-realtime-jni/src/lib.rs`: an independent Rust
  JNI cdylib which links the **same original** desktop `mesh-iroh` and
  `kart-wire` crates used in the Gate 1 standalone test. It maintains a
  Tokio runtime and a direct-only Iroh/MoQ endpoint in-process; repeated
  calls with the same key return the existing EndpointId. It refuses
  switching keys while an endpoint is running. This is *not* a race session.
- `android/turbo-kart-mobile/app/src/main/java/dev/holochain/turbokart/mobile/IrohIdentityStore.kt`:
  creates an independent 32-byte Ed25519 endpoint seed and encrypts it in
  app-private preferences under a non-exportable Android Keystore AES-GCM
  master key. It fails closed if ciphertext or the master key disappears;
  it does **not** share the Holochain Lair key.
- `IrohBridge.kt`: JNI loader; `MainActivity.kt`: separate
  **Start / Resume Iroh Endpoint (Test)** button enabled after the
  existing Holochain conductor starts.
- `scripts/build-turbo-kart-realtime-jni.sh`: builds the new ARM64
  `libturbo_kart_realtime_jni.so` via a generated isolated Cargo manifest
  using the unmodified desktop sources and the existing NDK r27c.
  Reuses Gate 1's build cache and verifies 16 KiB ELF LOAD alignment.
- `scripts/build-turbo-kart-apk.sh`: now stages and packages both separate
  JNI libraries. The existing H2HC library and original hApps are unchanged.

The isolated JNI dependency graph is intentional: the desktop game
uses different Holochain crates than the Android 0.7.1 host. Sharing
a single Cargo build graph could force unwanted dependency resolution.

Build and install on the known Linux workstation:

```sh
cd /mnt/holochain/TurboMobile/Holochain-mobile
git fetch origin feat/turbo-kart-integration
git merge --ff-only FETCH_HEAD

export PATH="$HOME/.local/opt/gradle-8.9/bin:$HOME/Android/Sdk/platform-tools:$PATH"
export ANDROID_HOME="$HOME/Android/Sdk"
bash scripts/build-turbo-kart-apk.sh

adb reverse tcp:8090 tcp:8090
adb install -r android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n dev.holochain.turbokart.mobile/.MainActivity
```

Acceptance procedure:

1. Tap **Start / Resume Holochain**; ensure `resumed:true` and the
   Holochain agent **matches the previous APK tests**.
2. Tap **Start / Resume Iroh Endpoint (Test)**. Record the complete
   `result.endpoint_id`; expect a nonempty `direct_addrs`, 42-byte
   `kart_wire_bytes`, and `signed_presence_published:false`,
   `authenticated_race_session:false`.
3. Run `adb shell am force-stop dev.holochain.turbokart.mobile`.
   Reopen the app, tap **Start / Resume Holochain** and then
   **Start / Resume Iroh Endpoint (Test)** again.
4. Require the **exact same Holochain agent and Iroh EndpointId** after
   process restart. Direct IP addresses/ports are allowed to change.
5. Ensure **Discover desktop lobbies**, **Join as Zippy** and **Leave**
   still work. **Do not clear app data or uninstall.**

The Iroh EndpointId inside this APK will **not** match Gate 1's ephemeral
adb-shell probe endpoint: the test-only executable deliberately generated
a temporary Iroh key and stored nothing. Gate 2a's Android Keystore secret
is separate and permanent across ordinary app restarts.

**Confirmed by on-device tests:** the same persistent Holochain agent
and Iroh EndpointId resumed across APK force-stop and restart.

## Gate 2b — published signed presence (source ready, device test pending)

The implementation uses the desktop project's `holochain-moq-protocol`
**unchanged** in the existing Iroh JNI library:

- `IrohBridge.signPresence(agent)` builds the original
  `SignedPresence` with an `EndpointKind::Iroh` address, the exact
  `iroh_binding_payload(endpoint_id, agent)`, an Ed25519 signature
  created by the persistent Iroh secret, and `PRESENCE_TTL_MS = 120000`.
  The endpoint secret **never leaves the Iroh JNI library**.
- `NativeBridge.publishSignedPresence(signedPresenceJson)` validates
  the advertised agent equals the installed Holochain conductor's
  persistent agent and checks the endpoint ID size, signature fields and
  timestamp. It calls the ORIGINAL gaming directory cell's
  `moq_directory.publish_signed_presence` and expects a Holochain
  `ActionHash`. The original DNA checks signature/authorship.
- It calls `moq_directory.get_endpoint_binding(ActionHash)` and requires
  the readback to equal the exact submitted presence before returning
  `readback_verified:true`.
- A **Verify directory Iroh bindings** button independently invokes
  `moq_directory.get_recent_endpoint_bindings` for the local agent
  and displays its newest records. No new DNA or Holochain capability,
  no new Android agent, and no different wire format were introduced.

**This version is not compiled or tested on the phone yet.** Rebuild and
install the updated APK using the same script and `adb install -r`;
never uninstall or clear saved data.

```sh
cd /mnt/holochain/TurboMobile/Holochain-mobile
git fetch origin feat/turbo-kart-integration
git merge --ff-only FETCH_HEAD
export PATH="$HOME/.local/opt/gradle-8.9/bin:$HOME/Android/Sdk/platform-tools:$PATH"
export ANDROID_HOME="$HOME/Android/Sdk"
bash scripts/build-turbo-kart-apk.sh
adb reverse tcp:8090 tcp:8090
adb install -r android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n dev.holochain.turbokart.mobile/.MainActivity
```

On the Galaxy S23:

1. Tap **Start / Resume Holochain** and confirm your original agent
   remains unchanged (`resumed:true`).
2. Tap **Start / Resume Iroh Endpoint (Test)** and confirm the original
   persistent Iroh EndpointId.
3. Tap **Publish signed Iroh presence**. The `ok:true` result MUST
   contain `signed_presence_published:true`,
   `readback_verified:true`, a `binding_action` and `expires_at_ms`.
   The `endpoint_fingerprint_hex` corresponds to the Iroh public key.
4. Tap **Verify directory Iroh bindings**; expect your new
   `binding_action` in the original Holochain directory's results.
   The verification is important because successful signing alone
   does not publish to the DHT.
5. Optional: force-stop/restart without clearing data, resume both keys,
   and use **Verify directory Iroh bindings** to verify the previously
   published binding is still readable. If it has expired, tap
   **Publish signed Iroh presence** again.

**Manual refresh only:** the binding expires 120 seconds after publishing,
so republish before the host attempts race setup. Automatic renewal,
Android background lifecycle and Wi-Fi/IP transitions are still missing.

**Do not start a desktop race with the phone yet.** It is not connected
to an authenticated MoQ session, so the existing desktop game will have
no Android racer to exchange frames with, even if signed presence succeeds.

## Gate 2c — locked race admission and authenticated MoQ (future)

Next, the desktop host's `start_race` can persist a locked roster
only once the phone's signed, nonexpired endpoint binding is readable.
Android must reuse `RaceAdmission::from_locked`, pinned endpoints,
the original iroh/MoQ session and the 42-byte WP3 snapshots. A lobby
join and valid signed presence are prerequisites, **not** proof of race
admission or frame transport. None of Gate 2c is implemented.

No Android race graphics, real-time peer session, host authority or
gameplay physics have been tested.
