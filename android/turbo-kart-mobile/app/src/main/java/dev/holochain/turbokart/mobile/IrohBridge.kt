package dev.holochain.turbokart.mobile

/**
 * Separate native library using the original desktop Iroh 1.2 / MoQ draft-16
 * transport and KartState wire codec. Does not touch the Holochain JNI host.
 *
 * The Kotlin caller retrieves a device-protected 32-byte seed and wipes its
 * temporary ByteArray after this synchronous native call returns.
 */
internal object IrohBridge {
    init {
        System.loadLibrary("turbo_kart_realtime_jni")
    }

    external fun startEndpoint(seed: ByteArray): String

    /** Signs the desktop domain-separated EndpointId -> agent binding. No zome write. */
    external fun signPresence(agent: String): String

    /** Calls the desktop's exact locked race admission validator; no peer connection yet. */
    external fun validateLockedAdmission(bundleJson: String): String

    /** Opt-in E6.2 MoQ smoke session: validates the locked roster before connecting. */
    external fun connectLockedRace(bundleJson: String): String

    /** Poll remote Iroh identity + matching admission digest; not race physics. */
    external fun raceStatus(): String

    /** Queue one synthetic 42-byte WP3 packet only after every peer authenticates. */
    external fun sendTestKart(): String

    /** Stop this race mesh without closing or regenerating the persistent endpoint. */
    external fun disconnectRace(): String
}
