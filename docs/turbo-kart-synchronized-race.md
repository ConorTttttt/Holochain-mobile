# Turbo Kart Android: synchronized race acceptance

## Result

**Two consecutive, complete desktop-versus-emulator races passed on the final dual-ABI APK.** The original desktop application created each lobby, Android joined as Zippy and published signed presence, desktop locked admission and entered through its normal Start Race / Enter race controls, and Android used Enter synchronized race. Both players ran the original countdown, physics, items, authoritative progress and finish logic. Both original result menus returned normally. The second race reused the same Android process, Holochain conductor and Iroh endpoint.

Work continued from verified, clean HEAD `d6d2e42ce39235860b0be0410bddb68c42d90460` on `feat/turbo-kart-mobile-combined`. The separate original worktree and its modified Cargo.lock were preserved. The desktop source at `/mnt/holochain/offloaded/sf/Turbokart/turbo-kart` remained at `8c83ed77a1b4`; no desktop source, DNA, zome, original rules, physics, touchscreen scheme or WP3 codec was rewritten.

## Implementation

The Android realtime JNI graph now links the **original `turbo-kart-sidecar` library** as well as the existing mesh crates. `production.rs` embeds its `Session` and `RaceCtl` using the existing persistent, Keystore-backed Iroh endpoint and original verified immutable admission. The original controller supplies peer authentication, WP4 clock exchange, readiness, one shared schedule, host authority, item channels, progress channels and session stop. The Wasmi/Holochain graph remains isolated. Existing diagnostics and the old development motion bridge remain available separately; they cannot overlap a synchronized native session.

`online-game.mjs` supplies a constrained capability-backed implementation of the desktop renderer's API. It invokes the original `Multiplayer.race()` / `enterRace()` and `startNetRace` path. Readiness is reported only after the original online scene is loaded. The renderer runs the original `NetRace`, host AI, progress tracker/view, `NetItems`, countdown and result menus. The original TKR2 binary envelope is transported through a bounded MessagePort bridge; kart payloads retain their original 42-byte WP3 encoding and actual simulation ticks. Signed identities and admission validation stay native. The capability exposes only fixed race operations and public admission data, never keys, files or arbitrary conductor commands. Local ACKs mean bounded local queue acceptance, not proof of receipt.

Android publication runs on a dedicated native thread with a 50 ms monotonic deadline. It consumes at most three queued actual physics samples, superseding older samples on overflow and skipping catch-up bursts after suspension. It never invents ticks or interpolates outgoing state. Reliable item/progress operations continue through the original session; local IPC IDs are rebased to preserve ordering when kart publication is delayed. Incoming original stamped deliveries retain authenticated agent attribution and use the original interpolation APIs. The original simulation timer continues independently of requestAnimationFrame.

The earlier renderer exit correlated with destroying the WebView when returning to MainActivity. The app now retains one empty reusable WebView: navigation to `about:blank` drops the game document and timers, closes the old capability and detaches the Activity context/clients. A new entry attaches fresh clients and a fresh capability. Explicit `onRenderProcessGone` handling also closes only the race and permits menu recovery. The old log's exit code alone does not prove the exact OS cause; the reproduced menu-return symptom did not recur during the two final races and subsequent single-player regression. Session stop drains the bridge worker, stops the original native race and its publication thread, and preserves the conductor and endpoint. MainActivity clears the completed local lobby selection so another lobby can be joined.

## Live evidence

All evidence here is under [evidence/synchronized](evidence/synchronized/). `race-3-result.json` and `race-4-result.json` were written by the **live** harness after each original race completed, not by revalidating the old development motion capture. Both races used Palm Cove, 50cc, one lap, two human players and six host-controlled AI. Automation used trusted CDP touch and Electron keyboard input, including FIRE on naturally obtained items. It did not set kart positions, progress, laps or inventory, and asserted that debug autopilot stayed disabled. The separate single-player FIRE regression does inject a local item to test its button; it is not multiplayer acceptance evidence.

| Final APK live run | Race 3 | Race 4, same process |
| --- | --- | --- |
| Both authoritative progress states | Completed | Completed |
| Eight final racer records identical | Passed | Passed |
| Desktop human | 3rd, 60.895 s | 1st, 58.403 s |
| Android human | 1st, 58.965 s | 8th, 64.718 s |
| Both humans FINISHED, not DNF | Passed | Passed |
| Opponents visible in both original cameras | Passed | Passed |
| Android item messages sent / received | 12 / 103 | 15 / 80 |
| Desktop item messages sent / received | 103 / 12 | 80 / 15 |
| Item refusals / dropped messages | 0 / 0 | 0 / 0 |
| Renderer game error registries | Both empty | Both empty |
| Original MAIN MENU on both clients | Passed | Passed |
| Native Android app PID | 3321 | 3321 |
| Renderer errors after menu marker | None | None |

Full admitted records, discovery/join/presence replies, intermediate progress samples, authenticated transport status, publication intervals, item counters, and final results are retained in the JSON files. Screenshots were captured independently from Android ADB and the original Electron window: `race-{3,4}-{android,desktop}-{countdown,driving,finished}.png`. Race 4's later Android result panel is `race-4-android-results.png`, with both result panels' text/state in `race-4-*-results-screen.json`. `race-*-menu-validation.json` and the corresponding logcat files verify menu return and session closure. Large logcat captures are losslessly gzip-compressed; inspect with `gzip -dc FILE.txt.gz`. The final desktop title also had no `__desktopMotion` or legacy `__emulatorPort` present.

Race 3 recorded 3 → 2 → 1 → GO on both clients. Race 4's observer attached during the final countdown second; it does not claim to have recorded the earlier Android marks. Both clients retained the same immutable schedule and the original native clock verdict was Good. The second race's actual GO events differed by **8.23 ms**, with approximately **0.96 ms observation uncertainty**. Android observed GO 21.98 ms after its deadline and desktop 14.31 ms after its deadline. [Clock evidence](evidence/synchronized/race-4-clock-validation.json) maps each page's monotonic event time to the same observer clock using the shortest CDP round trip. Emulator wall time lagged host wall time by about 760 ms, so subtracting raw cross-device `goWallMs` values would be invalid.

The final native publication windows had median intervals close to 50 ms, with means of 50.46 / 50.96 ms and p95 of 52.72 / 53.72 ms. During race 4's ten-second **15 FPS requestAnimationFrame probe**, the original simulation timer continued, Android rendered **132 frames**, and the native publisher sent **194 real physics states**, all **194 decoded by desktop**, over 10.063 s: **19.28 Hz**. The native thread reported zero skipped deadlines and no publication error. Occasional source/bridge stalls still produced maximum intervals around 400 ms; this is nominal 20 Hz with measured jitter, not a hard real-time guarantee. The probe restores the original renderer scheduling before race completion.

The saved public identities match the pre-update baseline and both final race setups:

- Holochain: `uhCAka2YIOVv2ENJLBgQTEuk7i5m_NwYfBVCwv7yTXjXYB9s3yDa5`
- Iroh: `53fdcfcfb33a25fab3f00464e4ee20c4f493a80127bc8b62e2ee26d9fc9b0d45`

Race 4's resume replies explicitly report Holochain `session_reused: true` and Iroh `already_running: true`; no regeneration or process restart occurred. The APK was installed with `-r`. No physical phone was contacted.

The first candidate's normal entry failed because its renderer API facade lacked the lobby-event method and returned the wrong race-get envelope. `race-1-entry-failure.json` is retained; the facade and its API regression test were corrected. Race 2 completed before the final pacing improvement; its evidence is retained separately. Final acceptance uses races 3 and 4 on the final APK.

## Build and tests

The existing 4 GiB build cap and 6 GiB runtime cap were preserved, with native compilation and emulator/desktop testing in separate phases. Both native ABIs compiled with `--locked`; package ID/launcher, full staged assets, ELF ABI and 16 KiB ELF/ZIP alignment checks passed. Runtime testing remained in the owned 6 GiB/no-swap scope, with the separate original-hApp authority helper capped at 1 GiB. No cap was increased. Both runtime cgroups recorded zero OOM events/kills. Owned runtime processes were stopped after collection; `runtime-stopped.txt` records both scopes inactive.

| Validation | Result |
| --- | --- |
| Touch/bridge JavaScript, including normal API contract and backpressure | 46 passed |
| Staging assertions | 3 passed |
| Original physics, surfaces, weapons, WP3 and interpolation | 62 passed |
| Android-native JNI codec/DTO assertions | 4 passed |
| Original sidecar binary/netmode/progress/racectl/realtime suites on Android | 39 passed, 1 optional relay-only test ignored |
| Final APK WebView single-player regression after two online races | 14 passed |
| Final APK original desktop-versus-emulator races | 2 completed, matching standings and menu return |

The sidecar tests include shared clock/readiness, admission/digest rejection, host-only AI/progress, original binary packets and endpoint reuse. The ignored relay-only probe requires `TK_RELAY_PROBE_URL`; live acceptance used the original HTTPS relay for authenticated rendezvous and then a direct path. Test logs are preserved alongside the race evidence. The 14 touchscreen checks cover real rendering/physics, countdown boost, automatic throttle, both steering directions, simultaneous touch pointers, original FIRE/camera behavior, pause/resume, background input release and returning to title.

APK: `android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk`

- Application ID: `dev.holochain.turbokart.mobile`
- Packaged native ABIs: `arm64-v8a`, `x86_64`
- APK SHA-256: `63b630c45d448480046dc6552f908b2740d478f6377963129a5f2a4338a93b73`
- Signing certificate SHA-256, unchanged: `71b3525b7af75fc11ff243f0a700b970d487b353eb867c11d4c14c7a75f72b38`

Rebuild with the existing SDK/NDK/JDK environment from the combined development guide:

```bash
bash scripts/build-turbo-kart-memory-safe.sh
```

To repeat acceptance, start the existing capped runtime and optional authority helpers, install with `adb -s emulator-5554 install -r` and configure the existing public H2HC linker/relay settings. The desktop must be at its normal title screen and Android at its native menu. Choose new evidence numbers to preserve existing recordings:

```bash
# Use Node 24 on PATH. Start the documented test stack, never during builds.
python3 scripts/emulator/prepare-normal-race.py --serial emulator-5554 --race 5
node --max-old-space-size=128 scripts/emulator/normal-race-test.mjs emulator-5554 race-5 --render-probe
node scripts/emulator/race-clock-evidence.mjs race-5
python3 scripts/emulator/finish-normal-race.py --serial emulator-5554 --race 5
python3 scripts/emulator/prepare-normal-race.py --serial emulator-5554 --race 6 \
  --same-process-as docs/evidence/synchronized/race-5-app-pid.txt
node --max-old-space-size=128 scripts/emulator/normal-race-test.mjs emulator-5554 race-6
python3 scripts/emulator/finish-normal-race.py --serial emulator-5554 --race 6
```

## Remaining scope

Physical Galaxy S23 acceptance remains unavailable. ARM64 compilation and packaging passed, but ARM64 device runtime is not newly claimed. Only a two-human, one-lap Palm Cove race was accepted live; other tracks/classes, longer races, cups and adverse-network/host-loss scenarios need their own live acceptance. The original online result panel explicitly says **PROVISIONAL FINISH ORDER · NOT RECORDED**: finishing positions synchronize, but results are not persisted to the DHT. Existing original mid-race recovery limits remain. Publication tolerates render changes but source/bridge stalls still cause occasional gaps. The menu renderer regression passed under this workload; it is not a long-duration renderer stress test.
