# Turbo Kart Android realtime phase — transport compatibility gate

**Status:** isolated Android Iroh/MoQ + WP3 probe code committed, **not yet
compiled or device-tested**. This is the first step beyond the verified
Galaxy S23 APK Holochain multiplayer lobby join/leave milestone.

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

## Gate 2 — authenticated Android endpoint, then race admission

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

No part of Gate 2 has been implemented. Full race movement, graphics,
host authority and recovery are subsequent gates. Keep the proven
Android Holochain APK unchanged while working on Gate 1.
