package dev.holochain.turbokart.mobile

import android.app.Activity
import android.os.Build
import android.os.Bundle
import android.view.View
import android.view.WindowInsets
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import org.json.JSONObject
import java.io.File
import java.util.concurrent.Executors

/**
 * First app-shell milestone: start/resume the native conductor and discover
 * desktop lobbies. Not a playable racing interface, and no real-time netcode.
 */
class MainActivity : Activity() {
    private val worker = Executors.newSingleThreadExecutor()
    private lateinit var linkerUrl: EditText
    private lateinit var linkerKey: EditText
    private lateinit var startButton: Button
    private lateinit var listButton: Button
    private lateinit var log: TextView
    private var started = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(24, 24, 24, 24)
        }
        val scroll = ScrollView(this)
        scroll.clipToPadding = false
        // targetSdk 35 uses edge-to-edge on modern Android. Respect the
        // status and navigation bars so the header and controls stay visible.
        scroll.setOnApplyWindowInsetsListener { view, insets ->
            val top: Int
            val bottom: Int
            if (Build.VERSION.SDK_INT >= 30) {
                top = insets.getInsets(WindowInsets.Type.statusBars()).top
                bottom = insets.getInsets(WindowInsets.Type.navigationBars()).bottom
            } else {
                @Suppress("DEPRECATION")
                top = insets.systemWindowInsetTop
                @Suppress("DEPRECATION")
                bottom = insets.systemWindowInsetBottom
            }
            view.setPadding(0, top, 0, bottom)
            insets
        }
        scroll.addView(column)
        setContentView(scroll)

        column.addView(TextView(this).apply {
            text = "Turbo Kart • Android H2HC / Wasmi"
            textSize = 22f
        })
        column.addView(TextView(this).apply {
            text = "APK prototype. Keep the desktop linker running and use adb reverse tcp:8090 tcp:8090."
        })

        linkerUrl = EditText(this).apply {
            setSingleLine(true)
            hint = "Linker WebSocket URL"
            setText("ws://127.0.0.1:8090/conductor/v1")
        }
        linkerKey = EditText(this).apply {
            setSingleLine(true)
            hint = "Padded base64 linker public key"
            setText("OC6JeMhmkekjeMjZZMpMzq2SVNUxkX/MSo2zWLIxufQ=")
        }
        column.addView(linkerUrl)
        column.addView(linkerKey)

        startButton = Button(this).apply {
            text = "Start / Resume Holochain"
            setOnClickListener { startConductor() }
        }
        listButton = Button(this).apply {
            text = "Discover desktop lobbies"
            isEnabled = false
            setOnClickListener {
                submit("Listing desktop lobbies…") {
                    NativeBridge.listOpenLobbies(10)
                }
            }
        }
        column.addView(startButton)
        column.addView(listButton)
        log = TextView(this).apply {
            text = "Ready to start. On first launch the two production hApps will install; later launches should resume the same agent."
            setTextIsSelectable(true)
        }
        column.addView(log)
    }

    private fun copyHapp(name: String): String {
        val dir = File(filesDir, "bundles")
        check(dir.exists() || dir.mkdirs()) { "Cannot create app-private bundle directory" }
        val file = File(dir, name)
        val tmp = File(dir, "$name.tmp")
        assets.open(name).use { source ->
            tmp.outputStream().use { destination -> source.copyTo(destination) }
        }
        check(tmp.renameTo(file)) { "Cannot stage $name bundle" }
        return file.absolutePath
    }

    private fun startConductor() {
        val url = linkerUrl.text.toString().trim()
        val key = linkerKey.text.toString().trim()
        submit("Starting native Wasmi conductor…") {
            val secret = PassphraseStore(applicationContext).getOrCreate()
            try {
                NativeBridge.start(
                    File(filesDir, "conductor").absolutePath,
                    url,
                    key,
                    copyHapp("gaming_platform.happ"),
                    copyHapp("turbo_kart.happ"),
                    copyHapp("expected-dna-hashes.json"),
                    secret
                )
            } finally {
                secret.fill(0)
            }
        }
    }

    private fun submit(message: String, job: () -> String) {
        startButton.isEnabled = false
        listButton.isEnabled = false
        log.text = message
        worker.execute {
            val result = try {
                job()
            } catch (error: Throwable) {
                JSONObject().put("ok", false)
                    .put("error", error.message ?: error.javaClass.simpleName).toString()
            }
            runOnUiThread {
                val parsed = try { JSONObject(result) } catch (_: Exception) { null }
                val ok = parsed?.optBoolean("ok") == true
                if (!started && ok && message.startsWith("Starting")) {
                    started = true
                }
                startButton.isEnabled = !started
                listButton.isEnabled = started
                log.text = parsed?.toString(2) ?: result
            }
        }
    }

    override fun onDestroy() {
        worker.shutdown()
        super.onDestroy()
    }
}
