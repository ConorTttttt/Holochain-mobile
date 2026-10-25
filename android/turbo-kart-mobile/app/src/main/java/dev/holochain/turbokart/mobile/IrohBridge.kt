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
}
