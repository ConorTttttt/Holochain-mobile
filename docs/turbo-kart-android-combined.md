# Turbo Kart Rally: combined Android development

Working branch: `feat/turbo-kart-mobile-combined`, based on WP3 `530fc87`, merged with `origin/feat/turbo-kart-touch-controls` (`4af7bc3`). The original worktree and its modified Cargo.lock are preserved.

## Build

Use the existing SDK, NDK r27c, JDK 17 and Gradle 8.9:

```bash
cd /mnt/holochain/TurboMobile/Holochain-mobile-combined
export ANDROID_HOME="$HOME/Android/Sdk"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export PATH="$HOME/.nvm/versions/node/v24.21.0/bin:$HOME/.local/opt/gradle-8.9/bin:$ANDROID_HOME/platform-tools:$PATH"
rustup target add aarch64-linux-android x86_64-linux-android
# Build both ABIs serially under the shared-host memory cap (default).
# One ABI: ANDROID_ABIS=x86_64 or arm64-v8a.
bash scripts/build-turbo-kart-memory-safe.sh
```

`ANDROID_SERIAL_MAKE=1` is an opt-in workaround for a reproduced Cargo/GNU Make 4.4.1 jobserver stall during nested OpenSSL builds. It runs Make serially, retaining Cargo caching, the original dependency graph and target link settings. It does not change system Make.

The Wasmi/Holochain and original desktop Iroh/MoQ graphs remain separate. Each ABI selects its own NDK API-26 clang linker, Cargo target directory and APK library directory. Both retain 16 KiB ELF LOAD alignment. The build stages the original desktop hApps, DNA hash manifest and original `resources/ui` runtime tree, then injects the touch and restricted gameplay adapters. Missing assets fail packaging. APK verification inspects the packaged ELF machine types, full staged runtime tree, app ID and launcher; zipalign checks ZIP placement.

Output: `android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk`.
Package: `dev.holochain.turbokart.mobile`; only `MainActivity` is exported as launcher. Single Player opens the original game in landscape. Multiplayer/Diagnostics retains native conductor, lobby, presence, locked admission and synthetic transport controls.

## Official emulator

The host is an Intel i5-12500T with 30 GiB RAM. KVM is available on the host; the restricted coding sandbox hides `/dev/kvm`. Run emulator/ADB commands from the host. No administrator package or group change was needed.

Official documentation: [SDK manager](https://developer.android.com/tools/sdkmanager), [emulator acceleration](https://developer.android.com/studio/run/emulator-acceleration), [emulator command line](https://developer.android.com/studio/run/emulator-commandline).

Only missing official SDK packages were installed, reusing Platform 35, Build Tools 35, platform-tools and the NDK:

```bash
"$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" \
  "emulator" "system-images;android-35;google_apis;x86_64"
"$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager" list device
# Only when this dedicated AVD is absent:
"$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager" create avd \
  -n TurboKart_API35 -k "system-images;android-35;google_apis;x86_64" -d pixel_8
bash scripts/emulator/start-turbo-kart.sh
```

The installed command-line tools still provide `sdkmanager` and `avdmanager`. They also provide the newer official `android sdk` / `android emulator` CLI. The named AVD was created using the available avdmanager interface and Pixel 8 profile. Current configuration: 2048 MiB RAM, two virtual CPUs, 6 GiB data partition, 720×1600 display at density 280, GPU auto, KVM required. The lower display allocation preserves the original layout dimensions while reducing graphics memory; the previous config is backed up as `config.before-memory-display.ini` beside this AVD's config. No unrelated AVD was modified. Hardware rendering uses the Intel UHD Graphics 770 through the emulator OpenGL translator; no software CPU or graphics fallback was used.

Validated serial: `emulator-5554`. It booted Android 15/API 35 and has Google WebView 124.0.6367.219. The image advertises `x86_64,arm64-v8a`; this project builds genuine x86_64 JNI libraries and does not rely on ARM translation.

```bash
adb devices -l
adb -s emulator-5554 shell getprop sys.boot_completed
adb -s emulator-5554 shell getprop ro.product.cpu.abilist
adb -s emulator-5554 shell getprop ro.build.version.sdk
adb -s emulator-5554 install -r android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk
adb -s emulator-5554 reverse tcp:8090 tcp:8090
adb -s emulator-5554 reverse --list
adb -s emulator-5554 shell am start -n dev.holochain.turbokart.mobile/.MainActivity
adb -s emulator-5554 logcat -d -v threadtime > target/emulator-logcat.txt
```

Always inspect and explicitly target the emulator serial. Never uninstall, clear data or copy phone identities. `scripts/emulator/ui.py` refuses physical-device serials and checks `ro.kernel.qemu` before UI automation.

## Gameplay bridge and limits

The WebView loads only `https://appassets.androidplatform.net/game/index.html`. File/content access is disabled. Requests outside the local assets are blocked, and CSP permits local asset fetches (needed by the original GLTF/audio loaders), blocks external connections and disallows child frames. Debug WebView inspection is limited to debug builds. A MessagePort capability is delivered only to the trusted main frame; navigation closes it. Gameplay accepts only status and bounded KartState DTO requests, never keys, files or conductor commands.

The development multiplayer entry requires an authenticated native session. The adapter samples the original `kartSnapshot` from actual player physics at 20 Hz, preserving the 60 Hz WP3 tick clock. Native code validates the original 42-byte codec and checks pinned peer authentication and the admission digest at each publish. Received states retain their authenticated agent identity, use bounded queues, and feed the original SnapshotBuffer and Kart.applyRemotePose implementation. Holochain stays outside this path. Synthetic tests are excluded once actual gameplay begins.

This bridge is a development motion integration. Shared host clock/readiness, authoritative race progress, synchronized items and full multiplayer lifecycle are still required before declaring a playable interoperable race. The game shows that limitation while the development bridge runs. Local publication acknowledgements never prove desktop receipt or rendering.

An optional HTTPS Iroh relay field uses the original desktop `Reach::DirectAndRelay` transport. Empty means direct UDP only. Changing it requires an application restart, preserving the same encrypted identity. The desktop's default game relay is `https://dev-test-bootstrap2.holochain.org/`; `--lan` disables it. ADB TCP reverse forwards H2HC only, never Iroh UDP. Emulator NAT is distinct from real Wi-Fi/cellular networking.

## Validation

Results, lifecycle evidence, screenshots and remaining blockers are recorded below after execution. Physical Galaxy S23 acceptance remains **PENDING**.

Automated checks (use Node 22+ for `--test-isolation=none`):

```bash
node --test mobile/touch-controls/*.test.mjs
node --test --test-isolation=none mobile/touch-controls/*.test.mjs mobile/game-bridge/*.test.mjs
python3 -m unittest discover -s mobile/touch-controls -p 'test_*.py'
bash scripts/test-turbo-kart-game.sh
python3 scripts/verify-turbo-kart-apk.py android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk --abis arm64-v8a,x86_64
```

The original game's existing 62 tests passed against the APK-staged source, using its vendored Three.js: real kart handling on track surfaces, all custom weapons, original WP3 golden bytes and remote interpolation. These are automated assertions without a renderer, separate from emulator gameplay checks.

Desktop test startup needed X11: Electron crashed after a Wayland/Vulkan incompatibility during its first launch. The existing source build then started normally with `env -u WAYLAND_DISPLAY XDG_SESSION_TYPE=x11 ... electron . --profile mobilebridge --lan`. Its separate development profile uses the original hApps and expected production DNA hashes. The test linker runs on 127.0.0.1:8090 attached to that conductor's admin port 40093, with a separate test linker key under ignored `target/emulator-linker/`. Only its public key is entered into the Android UI.

The main x86_64 Holochain/Wasmi JNI library compiled successfully with the original release settings and passed ELF machine/alignment checks. Its only dynamic dependencies are Android's libdl, liblog, libm and libc; no additional private .so is needed for this library. The separate Iroh/MoQ graph now has its own committed lockfile and builds with `--locked`, independently of the root Holochain 0.7.1 lockfile.

The touch implementation remains the touch-controls branch's manual countdown boost, invisible steering halves, automatic post-GO throttle, FIRE and held LOOK BACK scheme. Combined fixes add a reachable PAUSE button above the steering overlay and preserve an already-observed GO across an explicit pause while releasing all inputs. Automated touch/bridge assertions: 43 passed. Staging assertions: 3 passed. The adapter retains the admitted race class and advances beyond preceding diagnostic ticks on the same track.

Emulator-native Rust assertions: **4 passed**, executed as an x86_64 Android test binary under `/data/local/tmp/` (original codec round trips, bounded diagnostic cadence, strict gameplay DTO validation). Both JNI libraries also loaded through the normal app: Holochain started both original hApps, and Iroh started its native endpoint and original codec self-check.

Public emulator test identities (newly generated through Android Keystore/Lair):

- Holochain: `uhCAka2YIOVv2ENJLBgQTEuk7i5m_NwYfBVCwv7yTXjXYB9s3yDa5`
- Iroh: `53fdcfcfb33a25fab3f00464e4ee20c4f493a80127bc8b62e2ee26d9fc9b0d45`

Both survived an APK update with `adb -s emulator-5554 install -r`; see `evidence/combined/identity-apk-update.json`. No phone data or keys were used.

The first WebView run exposed blocked GLTF texture blobs. The policy now permits local asset and blob fetches while blocking external requests. The original Lilita One and Nunito fonts are bundled from Google's official font repository, with OFL licences and recorded SHA-256 hashes, to preserve the original menus offline. Source desktop files remain unchanged.

Single-player emulator acceptance: **14 checks passed** in the actual WebView, including original title/selection/track/karts, landscape WebGL 2, original physics motion, left/right steering, correctly timed original starting boost, automatic throttle handoff, FIRE invoking the original item system exactly once, real camera look-back/release, pause/resume, Android background cleanup, foreground resumption and returning through the original pause menu. The multitouch check sent two simultaneous CDP touch points, observed two distinct trusted `pointerType=touch` pointer IDs and verified steering plus camera input concurrently. It did not infer multitouch from sequential ADB taps.

Evidence: `evidence/combined/emulator-gameplay.json`, `emulator-gameplay.txt`, `main-menu.png`, `single-player-title.png`, `single-player-menu.png`, `track-loaded.png`, `kart-visible.png`, `touch-controls.png`, `gameplay-running.png`, `paused.png`, `returned-menu.png`.

Identity persistence: **emulator passed** for activity restart, application force-stop, emulator reboot and APK update. Both public IDs matched the baseline after every operation. See `identity-activity-restart.json`, `identity-force-stop.json`, `identity-emulator-reboot.json` and `identity-apk-update.json`. The reboot test needed a UI retry after boot settled; the final identity reads passed, and the reusable helper now waits for a changed kernel boot ID and completed boot before starting the Activity.

## Shared-host memory limits

The kernel recorded a host OOM event at 22:16 on 2026-10-27 that killed VSCode while the ARM64 build and test applications were running. The interrupted ARM64 build is not counted as passed. Subsequent native builds run separately from the emulator/desktop, with one Cargo job and a user-systemd scope limited to 4 GiB (`MemoryHigh=3G`, `MemoryMax=4G`, `MemorySwapMax=0`). The wrapper requires at least 8 GiB available before starting:

```bash
bash scripts/build-turbo-kart-memory-safe.sh
```

Gradle now defaults to a 768 MiB heap, one worker, no persistent daemon and in-process Kotlin compilation. The dedicated AVD now uses 2048 MiB RAM and two vCPUs; its data and identities were preserved. Do not run native compilation concurrently with emulator/desktop testing on this shared host.

Both ABIs subsequently compiled and the dual-ABI APK passed package/launcher, packaged ELF machine, asset-content and 16 KiB ZIP alignment checks under that capped build. Observed build memory near completion peaked at about 2.9 GiB. The final APK's signing certificate SHA-256 matches the original APK: `71b3525b7af75fc11ff243f0a700b970d487b353eb867c11d4c14c7a75f72b38`.

Start/stop the owned runtime test stack separately from builds:

```bash
bash scripts/emulator/start-memory-safe.sh
systemctl --user show turbo-kart-test-services.scope -p MemoryCurrent -p MemoryMax
# Before compiling again, stop only these owned test processes:
systemctl --user stop turbo-kart-test-services.scope
systemctl --user stop turbo-kart-test-authority.scope
```

The runtime stack has a shared 6 GiB hard limit and no swap allowance. It includes only the dedicated AVD, the desktop `mobilebridge` development profile and the dedicated test linker. Startup now requires 12 GiB available. An earlier 5 GiB cap terminated only the test group when both renderers loaded; see `evidence/combined/runtime-5g-limit.txt`. A 6 GiB run completed all 14 gameplay checks. A later simultaneous scene reset exceeded that cap too (`runtime-6g-scene-reset-limit.txt`); the limit again contained the failure within the test group. The AVD display was subsequently reduced, and the motion test reuses scenes instead of resetting both renderers together. A proposed increase to 7 GiB was rejected by automatic approval review and was not applied. Both startup/build wrappers check available memory and prevent heavy build/runtime phases from running together.

## DHT startup and real lobby acceptance

The fresh desktop profile and zero-arc mobile peer initially both advertised empty current storage arcs. The linker could exchange transport messages but had no DHT authorities to query. Starting a second normal, headless desktop conductor allowed full-target peers to converge to full current arcs. No DNA, validation, arc algorithm or phone identity was changed.

The optional helper runs unchanged original desktop hApps with a separate test identity and a 1 GiB hard limit. It uses its own marked directory under `/tmp/tk-combined-authority-1000/`; it never imports player keys. It requires Python PyYAML and the existing Node 24/client installation. Stop it before native builds:

```bash
bash scripts/emulator/start-authority-memory-safe.sh
systemctl --user show turbo-kart-test-authority.scope -p MemoryCurrent -p MemoryMax
systemctl --user stop turbo-kart-test-authority.scope
```

Actual emulator acceptance passed: desktop lobby discovery, native join as Zippy, independent desktop observation of Zippy in the rendered room, native leave, independent desktop observation of removal, rejoin, signed Iroh presence publication with signature/readback validation, directory binding readback and frozen two-player race admission. Evidence includes `final-discovery.json`, `desktop-joined.json/.png`, `desktop-left.json/.png`, `final-signed-presence.json`, `final-directory-bindings.json`, `final-locked-race.json` and `final-admission.json`.

The direct-only transport test explicitly started the desktop binary channel and `session_start`. The desktop was the deterministic dialer (`we_dial=true`), reached four attempts, then its 42-second dial deadline and reported `unreachable`. Android correctly remained the listener (`we_dial=false`, zero attempts). Its signed address hints were emulator guest `10.0.2.15/16` and `fec0::` addresses. No QUIC link, admission-digest exchange or WP3 delivery was established in that direct-only test. See `desktop-direct-status.json` and `android-direct-status.json`. ADB reverse was used only for H2HC TCP 8090.

The reusable runtime launcher now selects the original desktop HTTPS Iroh relay instead of `--lan`. Android's matching public relay setting requires a data-preserving app restart and fresh presence/admission. Set it before starting the endpoint. In the first relay test, both sides authenticated their pinned peer and matching admission digest; Android reported `authenticated_moq`, and desktop reported `connected`, no setup failures, and one dial attempt. After relay rendezvous, Iroh upgraded to a direct path. This proves direct UDP can work after discovery; it does not prove the emulator is directly reachable from its private advertised address alone. `reduced-binding.json` records the signed relay and discovered public UDP address. No insecure transport or admission bypass was used.

The bounded Android diagnostic completed 100 original 42-byte WP3 frames at nominal 20 Hz. Desktop received/decoded all 100 (`android-wp3-burst-complete.json`, `desktop-wp3-burst-complete.json`). Android also decoded original desktop WP3 states. These diagnostic counters are separate from the final actual-physics and camera evidence below.

The desktop motion harness (`scripts/emulator/desktop-motion-adapter.js`) runs in the unchanged source desktop page through its original API, physics, NetRace, codec and binary channel. It is explicitly a development test counterpart for the Android bridge, not the desktop's full online race controller. The host controller remains at clock synchronization: Android still needs the original readiness/clock, authoritative progress/items and full race lifecycle implementation. The live Android bridge targets at most 20 Hz; the first loaded dual-renderer run observed about 14–16 received frames/s, so its actual cadence is below the nominal target under this emulator workload. The standalone bounded diagnostic is separate from that render-dependent cadence.

## Final real-motion evidence and acceptance

The reduced-display run kept the 6 GiB hard cap and loaded each race scene once. Both original local physics simulations moved, both admitted remote models moved through the original interpolation/render APIs, and both opponents lay inside their original camera views. See `android-multiplayer-motion.png` and `desktop-multiplayer-motion.png`. The fresh capture recorded Android bridge counters of 82 actual physics states sent, 164 desktop states consumed and 269 remote render updates. Desktop Rust had published 333 states and decoded 97 Android states, with zero malformed or stale states at capture.

The original final harness mistakenly required more than 100 received states in this fresh race, although the separate synthetic burst was run in the earlier race. Its original failure and all captured data are retained in `multiplayer-motion.json` and `multiplayer-motion-original-harness.txt`. The corrected shared validator checks moving poses, admitted agent attribution, camera visibility, decoded frame counters and errors. Revalidating that unchanged recording passed **19 checks** (`multiplayer-motion-validation.json`); this is recorded-data validation, not a claim that the entire live harness was rerun after the threshold fix:

```bash
node scripts/emulator/motion-evidence.mjs \
  docs/evidence/combined/multiplayer-motion.json \
  docs/evidence/combined/multiplayer-motion-validation.json
```

Later independent native status still reported authenticated MoQ, **26,663 actual physics frames sent** and **22,645 desktop frames decoded** (`reduced-android-post-motion.json`). Final desktop counters and renderer state are in `reduced-desktop-post-motion.json`. Both renderer error registries were empty during the capture. After returning to MainActivity, WebView logged a renderer-process crash/exit with code -1; native status remained responsive. The cause of that exit needs further lifecycle investigation (`post-motion-webview-logcat.txt`); it is not counted as an in-race rendering failure or silently discarded.

The 14 single-player checks were run on the dual-ABI APK before the last bridge-only tick/class and menu-hint adjustments. Those changes passed the updated 43 JavaScript assertions; the final rebuilt APK was reinstalled with `-r`, passed complete package verification, and ran the admitted motion capture above. Its SHA-256 is recorded in `final-apk.sha256`. All owned emulator/desktop/linker/authority test processes were stopped after evidence collection to release host RAM. No phone was contacted or modified.

| Requested result | Recorded status |
| --- | --- |
| Branch | `feat/turbo-kart-mobile-combined` |
| Implementation commit | `5a5ea22f1407a62e657f817b919eef0b4007a0fc` |
| Emulator installed/name | Official Google Emulator; `TurboKart_API35`, Pixel 8 profile |
| Android/API/ABI/KVM | Android 15 / API 35 / native x86_64 / KVM enabled |
| APK | `android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk` |
| Application ID / packaged ABIs | `dev.holochain.turbokart.mobile` / `arm64-v8a`, `x86_64` |
| Launch / original single-player game | Emulator passed |
| Steering / auto acceleration / boost / FIRE / LOOK BACK | Emulator passed, including simultaneous pointers |
| Pause / resume / background input cleanup | Emulator passed |
| Holochain conductor / identity persistence | Emulator passed; activity restart, force-stop, reboot, APK update |
| Iroh endpoint / identity persistence | Emulator passed; same lifecycle operations |
| Desktop lobby discovery / join / leave | Emulator passed; independent desktop UI observation |
| Signed presence / locked race admission | Emulator passed; original signature and admission validators |
| Authenticated Iroh/MoQ / matching digest | Both peers passed with original HTTPS relay enabled; final path direct |
| Desktop receives Android WP3 | Passed: synthetic diagnostic and actual original physics |
| Android receives desktop WP3 | Passed: original desktop physics; native decode and remote interpolation |
| Opponent motion in both renderers | Development motion capture verified; both original camera views |
| Complete playable multiplayer race | **INCOMPLETE**: shared timing/readiness, authoritative progress/items and lifecycle remain |
| ARM64 compilation / signature compatibility | Passed; both original JNI graphs compiled, matching original signing certificate |
| Physical Galaxy S23 | **PENDING**, data and identities untouched |

Automated results: 43 JavaScript touch/bridge tests, 3 staging tests, 62 original-game physics/items/WP3/interpolation tests and 4 Android-native Rust assertions passed. The saved emulator game run has 14 passed checks; the separate recorded-motion validator has 19 passed checks.

Next work is to integrate the original race controller's clock/readiness and lifecycle, replace the development counterpart with the desktop's normal online entry, synchronize authoritative AI/progress/items, and decouple publication cadence from render frames. Investigate the WebView exit on returning to the native menu. A synchronized scored desktop/Android race and physical-device acceptance remain pending.

For later physical installation, first identify the actual phone serial; this command was **not** run on a phone:

```bash
adb -s S23_SERIAL install -r \
  android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk
```
