package dev.holochain.turbokart.mobile

/**
 * Calls Rust's in-process Wasmi/H2HC conductor. JNI calls are synchronous;
 * never invoke them on Android's main/UI thread.
 */
object NativeBridge {
    init {
        System.loadLibrary("turbo_kart_android_native")
    }

    external fun start(
        dataDir: String,
        linkerUrl: String,
        linkerKey: String,
        directoryHapp: String,
        kartHapp: String,
        expectedHashes: String,
        passphrase: ByteArray
    ): String

    external fun listOpenLobbies(limit: Int): String

    external fun getLobby(lobbyId: String): String

    /** Read original desktop locked race setup. Diagnostic only, not admission. */
    external fun getLockedRace(lobbyId: String): String

    /** Fetch immutable race record plus original signed directory bindings for realtime JNI verification. */
    external fun lockedAdmissionBundle(lobbyId: String): String

    external fun joinLobby(lobbyId: String, characterId: String): String

    external fun leaveLobby(lobbyId: String): String

    /** Publish a signed Iroh presence in the original gaming_platform.happ directory. */
    external fun publishSignedPresence(signedPresenceJson: String): String

    /** Independently query original directory entries; returns up to four newest bindings. */
    external fun recentSignedBindings(): String
}
