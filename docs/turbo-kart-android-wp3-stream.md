# Gate 2c.3 — measured WP3 stream from Galaxy S23 (experimental)

**25 October 2026: SOURCE-ONLY; NOT COMPILED OR DEVICE-TESTED.**

This branch is deliberately separate from the user’s current
`feat/turbo-kart-authenticated-moq` build. It extends that test with an
**explicit, bounded five-second burst** of the desktop game's own
42-byte `KartState` objects on MoQ `Latest` lane 4 (`state`).

- Branch: `feat/turbo-kart-mobile-wp3-stream`
- Depends on: Gate 2c.1 cryptographic locked-roster validation AND
  Gate 2c.2's actual Iroh/QUIC peer identity + matching admission
  digest. A signed presence alone is not a racing connection.
- Uses: the **same** Holochain agent, same saved Android Iroh EndpointId,
  same desktop `holochain-moq-kart-wire` codec, same already-open MoQ mesh.
- Never publishes Holochain zome calls from the streaming loop.
- No renderer, physics, steering, AI, authoritative race controller,
  player input, scoring or production gameplay is implemented.

## Controls

After the desktop has started a locked race and Gate 2c.2 reports
`authenticated_moq_peer_session:true`, Android enables:

**Send 20 Hz kart test (5s / 100 frames)**

This starts exactly one synthetic diagnostic burst per connected race
mesh. The sender uses `tokio::time::interval(50ms)` with
`MissedTickBehavior::Skip` so a paused Android process does not create
catch-up spikes. Every tick requires every roster peer to remain
connected and admitted. A failed send or dropped authentication
stops the burst and writes an explicit diagnostic error. Repeating the
burst requires closing and reconnecting a fresh authorized mesh.

Each WP3 sample has a monotonic `tick` (shared with the one-shot
test button), a small deterministic X-position displacement,
`vel=[0.4, 0, 0]`, no items and valid `track_t`. It is **synthetic**
and must never be mistaken for actual mobile gameplay.

**Check Iroh/MoQ connection status** returns:

```json
{
  "authenticated_moq_peer_session": true,
  "synthetic_kart_packets_sent": 100,
  "diagnostic_wp3_stream": {
    "test_only": true,
    "requested": true,
    "running": false,
    "completed": true,
    "nominal_hz": 20,
    "target_frames": 100,
    "sent": 100,
    "elapsed_ms": 4950,
    "error": null,
    "desktop_receipt_confirmed": false
  }
}
```

The JSON above is *illustrative*, **not an observed test result**.
Actual elapsed time may vary on Android. A `completed:true` result
means **100 packets were accepted by the local publisher**, not that
every one reached the desktop. The `Latest` transport may skip stale
packets by design; receiving all 100 is **not** a requirement.

## Prove desktop receipt independently

1. The desktop host and Galaxy S23 are in the **same immutable race**
   with both signed identities. On desktop, enter the race and confirm
   the original sidecar reports Zippy `connected:true`.
2. Use a controlled *test race*, not a competitive game in progress:
   synthetic WP3 coordinates will appear as if Zippy moved.
3. Obtain desktop's realtime sidecar status and note
   `kart.received` for the active race *before* the burst. The
   original sidecar has `kart_counters.received` and logs
   `kart_rx N (rate Hz)` in its five-second diagnostics.
4. Trigger the mobile burst. When done, poll
   **Check Iroh/MoQ connection status**; expect a local `sent=100`
   with `completed:true` and no error.
5. **Check desktop `kart.received` increased**, and/or the existing
   `kart_rx N` diagnostic shows actual packet arrivals for Zippy.
   The desktop's native E6.3 decoder and tick gate must accept frames.
   Do not count a local `send` return as peer delivery.
6. Note path `direct` or `relayed` and peer QUIC RTT.
   If the desktop doesn't report receipt, record the desktop sidecar
   logs and mobile `setup_errors`, connection status and stream stats.
7. Tap **Disconnect MoQ test (keep identity)**. Confirm packet
   generation stops without deleting Holochain or Iroh identity.
   Avoid clearing app data or uninstalling the APK.

## Build when Gate 2c.2 passes

This branch was created **after** the current build began. It is
deliberately not merged into the current branch.

```sh
cd /mnt/holochain/TurboMobile/Holochain-mobile
git fetch origin feat/turbo-kart-mobile-wp3-stream
git switch -c feat/turbo-kart-mobile-wp3-stream --track origin/feat/turbo-kart-mobile-wp3-stream
export ANDROID_HOME="$HOME/Android/Sdk"
export PATH="$HOME/.local/opt/gradle-8.9/bin:$HOME/Android/Sdk/platform-tools:$PATH"
bash scripts/build-turbo-kart-apk.sh
```

Only after a successful build:

```sh
adb reverse tcp:8090 tcp:8090
adb install -r android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n dev.holochain.turbokart.mobile/.MainActivity
```

If the same shell is currently running a Cargo/Gradle build, do NOT
switch branches until that build completes. Do not run both build
scripts against the same `target/` directory concurrently.

## Validation needed

Rust cross-compilation is not available in the assistant's environment,
so this branch requires a full local Linux workstation build. Unit
tests for the synthetic WP3 encoder are in
`prototypes/turbo-kart-realtime-jni/src/kart_stream.rs`. The isolated
JNI build script copies the module into its generated Cargo project
and continues to verify 16 KiB ELF alignment. The existing Android
APK script still performs the 16 KiB APK zipalign check. A passing
build is a prerequisite, not proof of networking.

The next milestone after real desktop receipt is proper Android
race-clock/controller integration, then actual kart position from a
mobile simulation or input-driven renderer. Synthetic data is not
playable racing.
