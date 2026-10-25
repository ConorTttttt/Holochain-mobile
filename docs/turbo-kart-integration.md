# Turbo Kart mobile Holochain smoke test

This is the **first integration milestone**, not an installable Android game. It
reuses the existing H2HC/Wasmi conductor, installs the two **unchanged** Turbo Kart
hApps with **one agent key**, compares their DNA hashes with the desktop
`expected-dna-hashes.json`, and offers read-only lobby discovery plus public-lobby join and leave commands.

Source: `prototypes/h2hc-conductor/src/bin/turbo-kart-smoke.rs`.
Nothing in the existing `h2hc-phone` benchmark, Turbo Kart desktop app or DNAs
is changed.

## Prepare on the Linux build machine

Obtain both hApp bundles and the expected-hashes JSON **from the same Turbo Kart
desktop build**. The desktop build script writes:

- `turbo-kart/apps/turbo-kart-desktop/.happs/gaming_directory/gaming_platform.happ`
- `turbo-kart/apps/turbo-kart-desktop/.happs/turbo_kart/turbo_kart.happ`
- `turbo-kart/apps/turbo-kart-desktop/expected-dna-hashes.json`

If the bundles do not already exist, consult the Turbo Kart desktop build
instructions (`yarn pack:happs`, requiring its sibling game source tree).
Do **not** casually rebuild or change the expected hashes: a different DNA hash
represents a different Holochain network.

First build the mobile project's pinned dependencies:

```sh
./scripts/vendor-holochain.sh
cargo build --release -p h2hc-conductor --bin turbo-kart-smoke --no-default-features --features wasmi
```

The H2HC linker must be running and attached to suitable full-arc authorities.
This standalone command does not start the linker or test the desktop
integration by itself.

Run the smoke harness on a **fresh disposable data directory**:

```sh
target/release/turbo-kart-smoke \
  --linker-url ws://127.0.0.1:8090/conductor/v1 \
  --linker-key '<base64-linker-public-key>' \
  --directory-happ /absolute/path/to/gaming_platform.happ \
  --kart-happ /absolute/path/to/turbo_kart.happ \
  --expected-hashes /absolute/path/to/expected-dna-hashes.json \
  --data-dir /tmp/turbo-kart-mobile-smoke-new
```

Use the linker's **actual** address and public key. After the `ready` JSON
message, type one JSON command per line:

```json
{"cmd":"install"}
{"cmd":"list_open_lobbies","limit":10}
{"cmd":"get_lobby","id":"<lobby-action-hash>"}
{"cmd":"join_lobby","id":"<lobby-action-hash>","character_id":"zippy"}
{"cmd":"get_lobby","id":"<lobby-action-hash>"}
{"cmd":"leave_lobby","id":"<lobby-action-hash>"}
{"cmd":"quit"}
```

The smoke harness joins **public** lobbies only. Joining and leaving write DHT records; leave the lobby before quitting. The test keystore does not persist its agent identity, and a restart requires a fresh data dir. With the corrected harness, `join_lobby`/`leave_lobby` return their action hashes; verify cross-agent membership using `get_lobby` and the desktop UI.

An `install` result with `both_enabled: true`, the same agent for both
apps, and matching DNA hashes is the first passing criterion. On 25 October 2026 a test running Holochain 0.7.1/Wasmi/H2HC listed a lobby created by the Holochain 0.7.0 desktop and retrieved its full `get_lobby` snapshot, including host, status, and members. Subsequently, the lightweight conductor wrote a successful `join_lobby` record for character `zippy`, the desktop UI independently showed the mobile-compatible player, and a `leave_lobby` record removed the player from the desktop UI.

## Android (physical device smoke test)

The existing `scripts/build-android.sh` still builds only the original
benchmarks. A **separate** `scripts/build-turbo-kart-android.sh` builds just
the Turbo Kart smoke harness using the same NDK setup and the Wasmi feature.
The Android Turbo Kart smoke binary was compiled and run on a Galaxy S23 on
25 October 2026. Both hApps installed with expected hashes; it successfully
listed and retrieved a desktop lobby, joined as Zippy, and the desktop UI
independently displayed its arrival and subsequent departure.

On the Linux host, with an Android ARM64 device authorised for ADB:

```sh
export PATH="$HOME/Android/Sdk/platform-tools:$PATH"
adb devices -l
adb shell getprop ro.product.cpu.abi
bash scripts/build-turbo-kart-android.sh
```

Expected binary:
`target/wasmi/aarch64-linux-android/release/turbo-kart-smoke`.

Keep the H2HC linker running on the host at `127.0.0.1:8090`. USB ADB
reverse redirects Android localhost port 8090 to the host's listener.

```sh
adb reverse tcp:8090 tcp:8090
adb reverse --list
BASE=/absolute/path/to/turbo-kart/apps/turbo-kart-desktop
PHONE=/data/local/tmp/turbo-kart-smoke
adb shell mkdir -p "$PHONE"
adb push target/wasmi/aarch64-linux-android/release/turbo-kart-smoke "$PHONE/turbo-kart-smoke"
adb push "$BASE/.happs/gaming_platform.happ" "$PHONE/gaming_platform.happ"
adb push "$BASE/.happs/turbo_kart.happ" "$PHONE/turbo_kart.happ"
adb push "$BASE/expected-dna-hashes.json" "$PHONE/expected-dna-hashes.json"
adb shell chmod 755 "$PHONE/turbo-kart-smoke"

# Replace with the public signing key from your currently running H2HC linker.
LINKER_KEY='<base64-linker-public-key-with-padding>'
RUN_DIR="$PHONE/data-$(date +%s)"
adb shell -T "$PHONE/turbo-kart-smoke \
  --linker-url ws://127.0.0.1:8090/conductor/v1 \
  --linker-key $LINKER_KEY \
  --directory-happ $PHONE/gaming_platform.happ \
  --kart-happ $PHONE/turbo_kart.happ \
  --expected-hashes $PHONE/expected-dna-hashes.json \
  --data-dir $RUN_DIR"
```

After `ready`, send newline-delimited JSON commands to stdin, beginning
with `{"cmd":"install"}`. For a fresh desktop public lobby, use
`list_open_lobbies`, `get_lobby`, `join_lobby`, `get_lobby`,
`leave_lobby`, then verify the desktop UI independently observed the
join and leave. A fresh `data-<timestamp>` dir is required for every new run.
Do not connect the test keystore to valuable player data.

## Security and limitations

- This harness uses Holochain's **in-memory test keystore**, so its keys do
  **not** survive restarts. Run on a fresh profile every time.
- It must **not** be used for production accounts or valuable player data.
- It is a command-line prototype; it is not an Android APK or iOS app.
- An opt-in Linux Lair persistence/resume experiment is documented in
  [turbo-kart-persistent-identity.md](turbo-kart-persistent-identity.md).
  This has **not been compile/run verified** or brought to the Android APK.
- Cross-version read interoperability (mobile Holochain 0.7.1 vs desktop 0.7.0) was observed for `list_open_lobbies` and `get_lobby`; cross-version join and leave writes were observed in the desktop UI. The harness incorrectly decoded the action-hash return value in that run; fixed by decoding it as `ActionHash`.
- It does not start races or implement Iroh/MoQ realtime sessions.
