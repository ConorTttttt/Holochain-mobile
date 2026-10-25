# Turbo Kart Android APK host — first JNI integration milestone

**Status (25 October 2026):** the JNI Rust library compiled for Android
arm64 with NDK r27c, the Kotlin/Gradle 8.9 debug APK built, and the Galaxy S23
ran the app successfully: the **Discover desktop lobbies** button returned
`ok:true` with desktop lobby `MobileHolochainTest`. This establishes the
APK's Kotlin → JNI → H2HC/Wasmi → desktop lobby read path. The app is still
not a playable mobile game, and its **persistent identity across Android app
process restarts has not yet been verified**.

The device also reported that the `libturbo_kart_android_native.so` ELF
LOAD segments were not 16 KiB-aligned. An APK build-script fix has been
committed using both NDK r27-compatible linker flags and an llvm-readelf
alignment check. **Rebuild and device verification of this fix remain pending.**

## Scope

The earlier adb-shell `turbo-kart-smoke` successfully installed the original
Turbo Kart hApps, discovered a desktop lobby, joined as Zippy and left again.
The Linux `--keystore lair` smoke harness separately verified persistent agent
identity across process restarts, including a working post-resume network read.

This APK prototype combines the ingredients in an **app-private Android
process** rather than starting a standalone executable under `adb shell`:

- `prototypes/turbo-kart-android-native`: Rust `cdylib` with JNI start and
  `listOpenLobbies` methods, an in-process H2HC/Wasmi conductor and guarded
  install/resume logic. Never installs/re-enables hApps before verifying both
  production DNA hashes and a shared agent.
- `android/turbo-kart-mobile`: simple Kotlin Android Activity with Start /
  Resume and Discover Lobbies buttons. Not a racing UI or an APK port of the
  desktop game's front end.
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

## Current limitations / safety

- **Not yet compiled or tested.** JNI symbol naming, Android Lair socket
  binding, app process lifecycle and battery/background handling require
  actual device verification. Never claim Android persistence is proven from
  the previous Linux result alone.
- Only `start` and read-only `listOpenLobbies` are exposed to Kotlin.
  There is no APK UI for join/leave yet. The separate ADB-shell harness
  already proved Android join/leave.
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
