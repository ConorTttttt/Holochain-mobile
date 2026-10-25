# Turbo Kart Android APK host — first JNI integration milestone

**Status (25 October 2026):** the JNI Rust library compiled for Android
arm64 with NDK r27c, the Kotlin/Gradle 8.9 debug APK built, and the Galaxy S23
ran the app successfully: **Discover desktop lobbies** returned `ok:true`
and found `MobileHolochainTest`. The user subsequently confirmed that
**all Android identity-restart checks passed**, including the same persistent
Holochain agent, hApp resume, and a successful post-restart lobby query.

The native library's ELF LOAD segments and the APK's ZIP placement passed
the 16 KiB alignment checks. After reinstalling the corrected APK, the user
also confirmed the previous Android **16 KiB compatibility warning was gone**.
This demonstrates alignment success on the tested device, not exhaustive
coverage of all 16 KiB-page hardware.

**The View Lobby / Join as Zippy / Leave Lobby APK update is now built,
installed and physically tested on the Galaxy S23.** The user independently
observed Zippy appear in the desktop Turbo Kart lobby on Android join, then
disappear on Android leave. This confirms cross-version Holochain **write
interoperability from the persistent Android APK UI**, not just the earlier
adb-shell test harness. The join/leave build retained the 16 KiB alignment
checks. The APK remains a multiplayer **control-plane prototype**, not a
playable racing game.

## Scope

The earlier adb-shell `turbo-kart-smoke` successfully installed the original
Turbo Kart hApps, discovered a desktop lobby, joined as Zippy and left again.
The Linux `--keystore lair` smoke harness separately verified persistent agent
identity across process restarts, including a working post-resume network read.

This APK prototype combines the ingredients in an **app-private Android
process** rather than starting a standalone executable under `adb shell`:

- `prototypes/turbo-kart-android-native`: Rust `cdylib` with JNI start,
  `listOpenLobbies`, `getLobby`, `joinLobby` and `leaveLobby` methods;
  an in-process H2HC/Wasmi conductor and guarded
  install/resume logic. Never installs/re-enables hApps before verifying both
  production DNA hashes and a shared agent.
- `android/turbo-kart-mobile`: Kotlin Android Activity with Start / Resume,
  Discover Lobbies, lobby selection, View Lobby, Join as Zippy and Leave Lobby
  controls. Not a racing UI or an APK port of the desktop game's front end.
- `PassphraseStore.kt`: generates a random 48-byte Lair secret, encrypts it
  using a non-exportable Android Keystore AES-GCM key, and stores only the
  ciphertext/nonce in app-private SharedPreferences. Does not silently mint
  new keys if stored conductor state or ciphertext cannot be recovered.
  `android:allowBackup=false` is intentional: cross-device recovery has not
  been designed.
- `scripts/build-turbo-kart-apk.sh`: cross-builds the arm64 JNI .so with the
  same NDK settings used for successful CLI tests, and stages exactly the
  desktop-built hApps and `expected-dna-hashes.json` into APK assets.

No changed DNAs, old benchmarks, desktop applications or CLI smoke files.

## Build on the Linux workstation

Prerequisites: the Android NDK r27c already used for `turbo-kart-smoke`;
Android SDK Platform 35; JDK 17; Gradle 8.9 (or an Android Studio installation
that can build the project). The project uses Android Gradle Plugin 8.7.3 and
Kotlin Android plugin 2.3.21. No Gradle wrapper JAR is committed.

```sh
cd /mnt/holochain/TurboMobile/Holochain-mobile
git fetch origin feat/turbo-kart-integration
git switch feat/turbo-kart-integration
git merge --ff-only FETCH_HEAD

# Build Rust JNI library and stage original desktop hApps. (No Gradle needed.)
bash scripts/build-turbo-kart-apk.sh --prepare-only

# Inspect native output:
file android/turbo-kart-mobile/app/src/main/jniLibs/arm64-v8a/libturbo_kart_android_native.so
```

After a successful Rust cross-build, the native library is available to the
Gradle project. On first build, Cargo may update `Cargo.lock` to include
JNI-related dependencies; keep the lockfile change in the working tree and
review before committing it.

For the debug APK, from the repository root:

```sh
# If Gradle 8.9 is installed in PATH:
bash scripts/build-turbo-kart-apk.sh

# Or open android/turbo-kart-mobile as an Android Studio project
# and run the Gradle assembleDebug task.
```

APK output:

```text
android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk
```

## Resolve the 16 KiB ELF compatibility warning

Android 15+ supports 16 KiB-page devices. NDK r27c normally generates
4 KiB-aligned ELF LOAD segments unless the linker receives both:

```text
-Wl,-z,max-page-size=16384
-Wl,-z,common-page-size=16384
```

The dedicated `scripts/build-turbo-kart-apk.sh` now sets these **only for
the aarch64 Android target** through
`CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS` and refuses to package
`libturbo_kart_android_native.so` unless every LOAD segment has 0x4000
alignment. This changes Cargo's build fingerprint and may require a
longer one-time rebuild of Android target dependencies. It does not
rebuild the benchmark harness.

```sh
git fetch origin feat/turbo-kart-integration
git merge --ff-only FETCH_HEAD
export PATH="$HOME/.local/opt/gradle-8.9/bin:$HOME/Android/Sdk/platform-tools:$PATH"
bash scripts/build-turbo-kart-apk.sh

NDK=${ANDROID_NDK_HOME:-$HOME/Android/Sdk/android-ndk-r27c}
"$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf" -lW \
  android/turbo-kart-mobile/app/src/main/jniLibs/arm64-v8a/libturbo_kart_android_native.so \
  | grep LOAD

# Reinstall without clearing app-private data:
adb install -r android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk
```

Expect 0x4000 alignment in the last column of every LOAD row. The app's
16 KiB page-size behavior is not fully proven until reinstalled and tested
on an actual 16 KiB-page device; correcting ELF/ZIP alignment is necessary
but not sufficient if any dependency assumes 4 KiB pages.

The debug-build compatibility warning may still mention that the app is
*debuggable*. That part is expected for this prototype.

## On the Galaxy S23

Keep the local linker running. ADB reverse is needed to use the default
`ws://127.0.0.1:8090/conductor/v1` URL. Cleartext transport is permitted
**only in the debug Android manifest**; a production deployment needs a
trusted TLS/WSS endpoint and correct network access rules.

```sh
export PATH="$HOME/Android/Sdk/platform-tools:$PATH"
adb devices -l
adb reverse tcp:8090 tcp:8090
adb reverse --list
adb install -r android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n dev.holochain.turbokart.mobile/.MainActivity
```

On-screen:
1. Check the linker URL and its **padded standard-base64 public key**.
2. Tap **Start / Resume Holochain**. First launch should report
   `both_enabled:true`, `resumed:false` and the **same two DNA hashes**
   as desktop.
3. Tap **Discover desktop lobbies** while a desktop public lobby is open.
   It should appear in the returned `lobbies` JSON.
4. Stop the Android app process without clearing app data, e.g.:
   `adb shell am force-stop dev.holochain.turbokart.mobile`.
   Re-open the Activity, tap Start / Resume, and compare agent IDs. A
   successful repeat must report `resumed:true`, same agent and DNA hashes,
   and a successful lobby read.

Do **not** run `adb shell pm clear` or uninstall the APK between persistence
tests. Those operations can destroy app-private conductor data and the
unlocking material. `adb install -r` is intended to keep app data.

## APK lobby membership controls (Galaxy S23 verified)

After pulling this branch and rebuilding with `scripts/build-turbo-kart-apk.sh`,
install the new APK using `adb install -r` (without clearing data), then:

1. Keep the desktop game in an open **public** lobby with a free player slot.
2. On Android, tap **Start / Resume Holochain**. Confirm `resumed:true`
   and the **same** Holochain agent as before.
3. Tap **Discover desktop lobbies** and select the intended lobby in the
   spinner (do not assume the first lobby is the right one).
4. Tap **View selected lobby** and check status/membership.
5. Tap **Join as Zippy**, check the returned `action` and independently
   confirm Zippy appears in the desktop game.
6. Tap **Leave joined lobby**, verify the action and confirm the desktop
   game removes Zippy.
7. Optionally force-stop/reopen the APK without clearing data, then select
   the lobby and tap **View selected lobby**. This also restores local
   knowledge of membership if the restarted app is still a member.

The native bridge reuses the exact `join_lobby` payload shape from the
successful adb-shell smoke test, including `invite_code: null`. Join/leave
results are decoded as Holochain `ActionHash` and returned as strings.
All JNI calls run on a background thread. The APK supports public lobbies
only; private invite codes are not yet implemented.

**Verified on Galaxy S23:** the built APK completed the UI join/leave
flow. The desktop game displayed Zippy arriving and departing in response
to the Android buttons, independently corroborating both native zome writes.
This test does not imply that the phone can start or render races yet.

## Current limitations / safety

- The APK's persistent-agent restart, post-resume lobby read, and
  **UI join/leave writes with desktop-visible results** have passed on
  the Galaxy S23. Extended lifecycle, recovery and battery tests remain.
- The UI supports public lobby join/leave using character `zippy` only.
  It does not implement private invites, character selection or race start.
- `ws://` cleartext and the test linker key are for local development only.
- The secret is held in process memory while unlocking Lair; Kotlin clears
  its temporary byte array afterward. Secrets are not committed or logged.
  Production hardening needs a formal security review, secure account
  recovery/migration, and measured startup/shutdown under Android lifecycle.
- No Iroh/MoQ race-data transport, gameplay controls, background service,
  notifications or production release signing.
- If the native conductor fails to start, gather a focused logcat trace:
  `adb logcat -d -t 300 | grep -Ei 'turbokart|holochain|lair|AndroidRuntime|UnsatisfiedLinkError'`.
  Do not delete saved player identity data as a workaround.

## Next milestone — authenticated real-time race admission

Desktop Turbo Kart already has authenticated iroh 1.2.0 / MoQ 0.3.10
race sessions, an immutable E6.1 race admission, and the E6.3
`KartState` snapshot track (42-byte WP3 format at 20 Hz). See the
desktop repository's `E61-RACE-START.md`, `E62-REALTIME-SESSIONS.md`
and `E63-KART-SYNC.md`. This is a separate native transport/data-plane;
H2HC and Holochain lobby writes are **not** the frame transport.

Recommended staged Android work:
1. Audit and reuse the existing `kart-net` / `mesh-iroh` /
   `RaceAdmission` code instead of inventing a second wire format.
2. Integrate a persistent Android-side iroh endpoint identity, advertise
   only valid signed presence, and validate locked race admission against
   the phone's existing persistent Holochain agent.
3. Add a mobile **race-ready** UI/diagnostic state, then prove one
   authenticated iroh/MoQ session with the desktop, with no racing
   physics yet. Do not claim admission merely because lobby join worked.
4. Reuse WP3 42-byte player snapshots and the existing game's physics
   and renderer/input adapters for a first visible two-device kart motion
   test; keep Holochain out of the high-frequency path.
5. Only after that expand to schedule, host authority, race progress,
   recovery, power, network transitions and gameplay UX.

This milestone is **not implemented or verified on Android yet**.
