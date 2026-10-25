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
}
