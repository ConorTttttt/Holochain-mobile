# Turbo Kart persistent identity: Linux verified, Android host next

The Android Wasmi/H2HC smoke test on 25 October 2026 succeeded: the
Galaxy S23 started both original hApps, discovered and read the desktop
`MobileHolochainTest` lobby, joined it as Zippy, and the desktop independently
displayed the join and later the leave.

On 25 October 2026, **persistent identity was verified on Linux** with the
Lair-backed smoke harness: the conductor was stopped after install and then
restarted from the same data directory and passphrase. The `resume` command
returned the **identical** agent key and the unchanged Turbo Kart DNA hashes,
with `both_enabled:true` and `resumed:true`. This result does **not** establish
persistent Android identity yet. The original Android smoke harness still uses
a disposable in-memory `DangerTestKeystore`.

## Stage 1: controlled Linux proof of key and conductor-state persistence

The `turbo-kart-smoke` binary now supports optional
`--keystore lair --passphrase-file FILE` and the JSON `resume` command.
This mode **compiled and passed the install/restart/resume identity check on
Linux on 25 October 2026**. It is still a development experiment, not a
production Android keystore integration. Existing calls without `--keystore` retain the original disposable
memory mode. Never reuse a data dir created with the memory keystore.

Build on the Linux PC:

```sh
cd /mnt/holochain/TurboMobile/Holochain-mobile
cargo build --release -p h2hc-conductor --bin turbo-kart-smoke \
  --no-default-features --features wasmi
```

Prepare a new, access-restricted profile and keep its passphrase for every
future restart. Do not share the passphrase or commit it to Git.

```sh
BASE=/mnt/holochain/offloaded/sf/Turbokart/turbo-kart/apps/turbo-kart-desktop
PERSIST="$HOME/.local/state/turbo-kart-h2hc-persist-test"
LINKER_KEY='REPLACE_WITH_CURRENT_LINKER_PUBLIC_KEY_PADDED_BASE64'
mkdir -p "$PERSIST"
chmod 700 "$PERSIST"
if [ ! -e "$PERSIST/keystore.pass" ]; then
  (umask 077; head -c 48 /dev/urandom | base64 | tr -d '\n' > "$PERSIST/keystore.pass")
fi
chmod 600 "$PERSIST/keystore.pass"
```

Start the smoke harness, using the same `PERSIST/data` path on **both** runs.
Keep the H2HC linker online.

```sh
./target/release/turbo-kart-smoke \
  --linker-url ws://127.0.0.1:8090/conductor/v1 \
  --linker-key "$LINKER_KEY" \
  --directory-happ "$BASE/.happs/gaming_platform.happ" \
  --kart-happ "$BASE/.happs/turbo_kart.happ" \
  --expected-hashes "$BASE/expected-dna-hashes.json" \
  --data-dir "$PERSIST/data" \
  --keystore lair \
  --passphrase-file "$PERSIST/keystore.pass"
```

First run: type `{"cmd":"install"}`, record the returned `agent`, and
`{"cmd":"quit"}`. Restart the **same command** with the **same** profile and
passphrase, then type `{"cmd":"resume"}` (not `install`).
**Verified:** `resumed:true`, `both_enabled:true`, the exact same agent
public key and both original production DNA hashes. The agent ID returned on
both runs was
`uhCAksfr6CgmhV3uu-WrNFRotvSHK06CY_KsObaXL0OQapv0nB30g`.
The user subsequently ran `{"cmd":"list_open_lobbies","limit":10}` after
`resume` and received `ok:true`, including the existing desktop
`MobileHolochainTest` lobby. **Post-resume network reads are verified.**
Capture any future errors instead of resetting this persistent test profile. Do not run two conductors pointing at the
same data directory concurrently.

## Stage 2: Android APK-private data and key management

Do **not** try to make the `adb shell` smoke harness production-persistent
by simply setting `--keystore lair`. The existing `h2hc-phone` code notes
that the Android shell UID cannot bind Lair's Unix socket in that environment.
The native APK host uses app-private storage, a local Lair keystore
and a random Lair-unlock secret encrypted by Android Keystore. The first
installed APK was built and run on a Galaxy S23, and the user confirmed
that **all post-restart tests passed**: identical Android agent ID,
`resumed:true`, both original hApps enabled and a working desktop lobby
query after force-stop/relaunch. These results establish identity persistence
in the **tested Android APK configuration**, not general production readiness.

The user also confirmed that the corrected NDK r27c 16 KiB-aligned binary
and Gradle APK build passed the respective alignment checks and that the
device no longer displayed its earlier 16 KiB compatibility warning.

Keep the earlier disposable Android adb-shell smoke harness available as a
regression test; it intentionally does not share the APK's identity.

### Stage 3: Android lobby controls and remaining security work

New Kotlin controls and Rust JNI methods for `get_lobby`, `join_lobby`
and `leave_lobby` have been committed. They have **not yet been compiled
or device-tested in an updated APK**. See
[turbo-kart-android-apk.md](turbo-kart-android-apk.md) for the phone acceptance
procedure. A successful APK test requires independent desktop UI evidence.

Recovery/backup and migration of keystore-protected agent identity across
devices, Android background execution, auth/transport hardening, and racing
data-plane (Iroh/MoQ) integration remain future work.
