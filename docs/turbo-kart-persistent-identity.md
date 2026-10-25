# Turbo Kart persistent identity: the next integration milestone

The Android Wasmi/H2HC smoke test on 25 October 2026 succeeded: the
Galaxy S23 started both original hApps, discovered and read the desktop
`MobileHolochainTest` lobby, joined it as Zippy, and the desktop independently
displayed the join and later the leave.

The next milestone is **same agent after conductor restart**. The original
smoke harness uses Holochain's `DangerTestKeystore` and makes a new test
identity. A successful multiplayer session does not prove identity durability.

## Stage 1: controlled Linux proof of key and conductor-state persistence

The `turbo-kart-smoke` binary now supports optional
`--keystore lair --passphrase-file FILE` and the JSON `resume` command.
This is an **unverified development experiment**, not a production keystore
integration. Existing calls without `--keystore` retain the original disposable
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
Pass criteria: `resumed:true`, `both_enabled:true`, the **exact same**
agent public key and original production DNA hashes, and a working
`list_open_lobbies` read. This is not yet proven; capture build/run errors
rather than resetting the profile. Do not run two conductors pointing at the
same data directory concurrently.

## Stage 2: Android APK-private data and key management

Do **not** try to make the `adb shell` smoke harness production-persistent
by simply setting `--keystore lair`. The existing `h2hc-phone` code notes
that the Android shell UID cannot bind Lair's Unix socket in that environment.
The next Android implementation needs a native Android host process with an
app-private data directory and verified Lair socket support, or a different
properly integrated persistent keystore. Securely derive/store the Lair
unlocking material through Android Keystore (not a fixed password in source,
not a passphrase pushed through ADB); explicitly plan backup/recovery.

The APK host must implement the same `install`/ `resume` identity reuse
semantics and ensure exactly the two production DNA hashes, independently of
the game's realtime Iroh/MoQ racing data plane.

Keep the working disposable Android smoke harness available as a regression
test. Neither this stage nor the persistent Linux prototype should be
described as an Android player-identity feature until built and tested on
the physical device.
