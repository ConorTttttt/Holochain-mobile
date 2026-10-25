# Gate 2c.2 — Android authenticated Iroh/MoQ peer smoke test

**Status (25 October 2026): implementation source committed on a SEPARATE
experimental branch, `feat/turbo-kart-authenticated-moq`; not compiled,
installed, or device-tested.** The stable APK feature branch
`feat/turbo-kart-integration` is deliberately unchanged while the user's
Gate 2c.1 build is running. Do not claim an authenticated session until a
real desktop sidecar and Galaxy S23 both report the matching handshake.

## Scope

This stage uses the *existing* Android Keystore-preserved Ed25519 Iroh
EndpointId and Holochain agent. The original Holochain JNI host obtains
the committed `get_race` E6.1 `RaceRecord` and every participant's exact
`moq_directory.get_endpoint_binding` signed action. The isolated Iroh JNI
library reruns the **unchanged**
`holochain_moq_turbo_kart_client::race::RaceAdmission::from_locked`
validator and checks that:

- Android's running Iroh EndpointId exactly matches its frozen signed binding;
- every remote dial hint decodes to its frozen EndpointId;
- no peer connects without an authenticated Iroh/QUIC endpoint identity
  that matches the immutable mesh roster;
- the remote publishes the **same SHA-256 E6.2 admission digest** before
  being counted as authenticated.

For a matching roster, `MeshSession<IrohMeshTransport>` starts over the
**same app-private endpoint** using `from_shared_endpoint`, not a newly
generated or temporary Iroh identity. It reproduces desktop sidecar tracks:

| Lane | Track | Delivery |
| --- | --- | --- |
| 0 | `e62-test-admission` | Reliable ordered; mandatory digest |
| 1 | `e62-test-latest` | Latest |
| 2 | `e62-test-events` | Reliable ordered |
| 3 | `e62-test-host` | Host AI-0 source; host only |
| 4 | `state` | Latest; desktop WP3 KartState |

Desktop `realtime.rs` is the source of truth for the track layout,
admission digest, 16 initial dial attempts, 2-second retry and 42-second
dial deadline. The Android side reuses `mesh-iroh`, `mesh`,
`turbo-kart-client`, and `kart-wire` from the original desktop game
source directory.

## New APK controls (unverified)

- **Connect Iroh/MoQ (test)**: after Holochain and the persistent Iroh
  endpoint start, and after a **locked** race is readable, fetch & validate
  E6.1 race setup and asynchronously start the real peer mesh. `ok:true`
  from this button only means mesh setup was attempted; it is **not yet
  an authenticated connection**.
- **Check Iroh/MoQ connection status**: read actual mesh connections,
  Iroh QUIC RTT/path, dial failure and digest verification. Require
  `state: "authenticated_moq"`,
  `authenticated_moq_peer_session: true`, and the correct roster size
  **only after both peers are actually connected and have exchanged
  identical admission digests**.
- **Send ONE test kart packet (42 bytes)**: enabled only after a
  successful status poll; native code independently rechecks every
  connection before sending one synthetic WP3 `KartState` to the real
  authenticated `race/<race>/<agent>/state` track. `ok:true` proves
  it was accepted by the *local MoQ publisher*, **not that the desktop
  received it**. Confirm receipt separately using the desktop sidecar's
  kart receive counter/log.
- **Disconnect MoQ test (keep identity)**: stop race-scoped tracks and
  sessions; leave both protected Holochain and Iroh identities alive.

**Not implemented:** authoritative host/clock control, a race-ready
handshake, renderer, steering/physics, position replication at 20 Hz,
background recovery, a guaranteed receive acknowledgement, and any
Android race simulation. `race_control_active:false` and
`authenticated_race_session:false` remain deliberately false even when
`authenticated_moq_peer_session:true`. This is a peer transport gate,
not a playable racing client.

## Build only after the current Gate 2c.1 compile is complete

Do **not** pull this experimental branch into the stable branch while
the current build is running. Review the source diff first.

```sh
cd /mnt/holochain/TurboMobile/Holochain-mobile
git fetch origin feat/turbo-kart-authenticated-moq
git switch -c feat/turbo-kart-authenticated-moq --track origin/feat/turbo-kart-authenticated-moq
export ANDROID_HOME="$HOME/Android/Sdk"
export PATH="$HOME/.local/opt/gradle-8.9/bin:$HOME/Android/Sdk/platform-tools:$PATH"
bash scripts/build-turbo-kart-apk.sh
```

The isolated realtime JNI graph now includes the original desktop
`holochain-moq-turbo-kart-client` crate. It may compile more dependencies
or encounter Android-specific errors; capture the first Rust failure.
The cross-build still checks the 16 KiB ELF LOAD alignment, and APK
packaging checks 16 KiB ZIP placement.

**After** `BUILD SUCCESSFUL`:

```sh
adb reverse tcp:8090 tcp:8090
adb install -r android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n dev.holochain.turbokart.mobile/.MainActivity
```

`adb install -r` preserves saved identities. Never uninstall the app,
clear Android app data, delete Lair storage, or reset the Keystore.

## Integration acceptance procedure

1. The desktop host keeps the actual **same-DNA** Turbo Kart lobby
   running, including Zippy. Both desktop and Android publish fresh
   **signed** Iroh presence; the phone's directory write expires after
   120 seconds and the desktop host requires at least 30 seconds remaining.
2. The host starts an immutable E6.1 race with both players in its
   original workflow. Read `get_race` on the phone and confirm Zippy
   appears in the frozen roster. **Do not start a race on the phone.**
3. Enter the desktop race so its normal sidecar starts an E6.2 MoQ
   session. On the Galaxy S23 press **Verify signed race admission** and
   then **Connect Iroh/MoQ (test)**.
4. Press **Check Iroh/MoQ connection status**. Look for
   `authenticated_moq_peer_session:true`, matching `race_id`,
   the expected desktop `agent`, `connected:true` and realistic RTT.
   This must be corroborated by desktop sidecar peer status, not just
   a mobile JSON result. The desktop may report race-control waiting
   for a full Android racer: that is not a peer-authentication failure.
5. **Only after verified peer connectivity**, press the one-packet
   button. Expect a 42-byte WP3 packet and confirm the desktop
   sidecar's kart receive count increases. A local send acknowledgement
   is not a remote receipt acknowledgement.
6. Disconnect MoQ, force-stop/reopen APK, resume both protected
   identities, and verify the exact same keys still exist.
   An old E6.1 race may not be reusable for further tests; use the
   desktop's normal race/lobby lifecycle.

## Failure cases to capture

- `no locked race`: the host must complete E6.1; neither signed
  presence nor lobby membership alone starts a race.
- `RaceAdmission::from_locked rejected`: keep the original validation
  intact and examine which frozen binding/signature/agent mismatched.
- `initial authenticated dial ...` or 45-second timeout: the desktop
  sidecar may not have entered the race, the direct LAN path may be
  unreachable, or the peer may have rejected the frozen identity.
- `admission digest differs`: immutable setup, roster order or
  endpoint identity disagrees; leave/enter via the desktop's original
  workflow. Do not disable authentication.
- `cannot publish synthetic WP3 ...`: reconnect, poll status, and
  check the desktop's reciprocal admission digest; never bypass the
  native gate.

This branch deliberately does not alter the desktop repository, DNA
bundles, the on-device app-private identities, or current Gradle/NDK
settings.
