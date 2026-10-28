package dev.holochain.turbokart.mobile

import android.app.Activity
import android.graphics.Color
import android.os.Bundle
import android.net.Uri
import android.util.Log
import android.webkit.ConsoleMessage
import android.webkit.WebMessage
import android.webkit.WebMessagePort
import org.json.JSONObject
import java.util.concurrent.Executors
import android.view.View
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.TextView
import java.io.ByteArrayInputStream
import java.io.FileNotFoundException

/**
 * Standalone landscape single-player playtest using Turbo Kart's packaged
 * Three.js game and a multi-touch keyboard adapter. No privileged JS bridge:
 * the game gets neither conductor credentials nor Android filesystem access.
 */
class TouchGameActivity : Activity() {
    private var game: WebView? = null
    private var port: WebMessagePort? = null
    private val bridgeWorker = Executors.newSingleThreadExecutor()
    private var paused = false
    private var portGeneration = 0
    private val synchronizedRace get() = intent.getBooleanExtra("synchronizedRace", false)
    private val inFlight = java.util.concurrent.atomic.AtomicInteger(0)
    private var closing = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val available = try {
            assets.open("game/index.html").use { true }
        } catch (_: FileNotFoundException) { false }
        if (!available) {
            setContentView(TextView(this).apply {
                text = "Touch game assets have not been staged. Set TURBO_KART_GAME_UI_DIR and rebuild the debug APK."
                textSize = 20f
                setPadding(30, 30, 30, 30)
            })
            return
        }
        @Suppress("DEPRECATION")
        window.decorView.systemUiVisibility = (
            View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or
            View.SYSTEM_UI_FLAG_FULLSCREEN or
            View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY or
            View.SYSTEM_UI_FLAG_LAYOUT_STABLE
        )

        val view = GameWebViewStore.obtain(this)
        view.setBackgroundColor(Color.BLACK)
        view.isLongClickable = false
        view.settings.apply {
            javaScriptEnabled = true
            domStorageEnabled = true
            allowFileAccess = false
            allowContentAccess = false
            javaScriptCanOpenWindowsAutomatically = false
            setSupportMultipleWindows(false)
            mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
        }
        WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)
        view.webChromeClient = object : WebChromeClient() {
            override fun onConsoleMessage(message: ConsoleMessage): Boolean {
                Log.i("TurboKartWeb", "${message.messageLevel()}: ${message.message()} (${message.sourceId()}:${message.lineNumber()})")
                return true
            }
        }
        view.webViewClient = object : WebViewClient() {
            override fun onRenderProcessGone(view: WebView, detail: android.webkit.RenderProcessGoneDetail): Boolean {
                Log.e("TurboKartWeb", "Active renderer gone; didCrash=${detail.didCrash()}, priority=${detail.rendererPriorityAtExit()}")
                portGeneration++; port?.close(); port = null
                (view.parent as? android.view.ViewGroup)?.removeView(view)
                view.destroy(); game = null
                if (synchronizedRace) bridgeWorker.execute { IrohBridge.productionRequest("{\"op\":\"stop\"}") }
                setContentView(TextView(this@TouchGameActivity).apply { text = "Game renderer stopped. Use Android Back to return to the app menu; your player identities are preserved." })
                return true
            }
            override fun onPageStarted(view: WebView, url: String, favicon: android.graphics.Bitmap?) {
                portGeneration++
                port?.close(); port = null
            }
            override fun onPageFinished(view: WebView, url: String) {
                if (url != GAME_URL || !(intent.getBooleanExtra("multiplayer", false) || synchronizedRace)) return
                val channels = view.createWebMessageChannel()
                port = channels[0]
                val generation = portGeneration
                channels[0].setWebMessageCallback(object : WebMessagePort.WebMessageCallback() {
                    override fun onMessage(channel: WebMessagePort, message: WebMessage) {
                        val raw = message.data ?: return
                        if (raw.length > 8192 || generation != portGeneration) return
                        val request = try { JSONObject(raw) } catch (_: Exception) { return }
                        val requestId = request.optLong("id", -1)
                        if (requestId < 1) return
                        if (synchronizedRace && request.optString("op") == "exit") { finish(); return }
                        if (paused) {
                            channel.postMessage(WebMessage(JSONObject().put("id", requestId).put("ok", false).put("errorCode", "paused").toString()))
                            return
                        }
                        if (inFlight.incrementAndGet() > 24) {
                            inFlight.decrementAndGet()
                            channel.postMessage(WebMessage(JSONObject().put("id", requestId).put("ok", false).put("error", "Race bridge backpressure").toString()))
                            return
                        }
                        bridgeWorker.execute {
                            val result = try {
                                when (request.getString("op")) {
                                    "race" -> {
                                        check(synchronizedRace)
                                        IrohBridge.productionRequest(request.getJSONObject("request").toString())
                                    }
                                    "status" -> { check(!synchronizedRace); IrohBridge.raceStatus() }
                                    "frame" -> { check(!synchronizedRace); IrohBridge.exchangeGameState(request.getJSONObject("state").toString()) }
                                    else -> "{\"ok\":false,\"error\":\"Unknown gameplay operation\"}"
                                }
                            } catch (_: Exception) { "{\"ok\":false,\"error\":\"Invalid gameplay request\"}" }
                            inFlight.decrementAndGet()
                            runOnUiThread {
                                if (generation == portGeneration && port === channel) channel.postMessage(WebMessage(JSONObject(result).put("id", requestId).toString()))
                            }
                        }
                    }
                })
                // This capability is transferred only to our trusted main frame.
                val token = java.util.UUID.randomUUID().toString()
                view.evaluateJavascript("window.__turboKartGamePortToken = ${JSONObject.quote(token)};", {
                    if (generation == portGeneration && view.url == GAME_URL) {
                        val prefix = if (synchronizedRace) "turbo-kart-online-port" else "turbo-kart-game-port"
                        view.postWebMessage(WebMessage("$prefix:$token", arrayOf(channels[1])), Uri.parse("https://$ASSET_HOST"))
                    } else { channels[1].close() }
                })
            }
            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                // Never navigate to arbitrary external pages from the game screen.
                return request.url.scheme != "https" || request.url.host != ASSET_HOST
            }

            override fun shouldInterceptRequest(view: WebView?, request: WebResourceRequest?): WebResourceResponse? {
                val url = request?.url ?: return null
                if (url.scheme != "https" || url.host != ASSET_HOST) return missing()
                val path = url.path ?: return missing()
                if (!path.startsWith("/game/")) return missing()
                val relative = path.removePrefix("/game/")
                if (relative.isEmpty() || relative.split('/').any { it == ".." || it == "." }) return missing()
                return try {
                    WebResourceResponse(mime(relative), if (isText(relative)) "UTF-8" else null,
                        200, "OK", mapOf("Content-Security-Policy" to
                            "default-src 'self' data: blob:; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self' blob:; frame-src 'none'; object-src 'none'; base-uri 'self'"),
                        assets.open("game/$relative"))
                } catch (_: Exception) { missing() }
            }
        }
        game = view
        setContentView(view)
        view.loadUrl(GAME_URL)
    }

    override fun onPause() {
        paused = true
        game?.evaluateJavascript("window.dispatchEvent(new Event('blur'));", null)
        game?.onPause()
        super.onPause()
    }
    override fun onResume() {
        super.onResume(); paused = false; game?.onResume()
        game?.evaluateJavascript("window.dispatchEvent(new Event('focus'));", null)
    }
    override fun onDestroy() {
        portGeneration++; port?.close(); port = null
        if (synchronizedRace) bridgeWorker.execute { IrohBridge.productionRequest("{\"op\":\"stop\"}") }
        bridgeWorker.shutdown()
        game?.let { GameWebViewStore.release(it, this) }; game = null; super.onDestroy()
    }

    @Suppress("DEPRECATION")
    override fun onBackPressed() {
        if (!synchronizedRace) { super.onBackPressed(); return }
        if (closing) return
        closing = true
        game?.evaluateJavascript("window.__turboKartOnlineSession?.leave().catch(console.error);", null)
        // Also covers Back during initial connection or a lost renderer.
        android.os.Handler(mainLooper).postDelayed({
            if (!isFinishing && !isDestroyed) bridgeWorker.execute {
                IrohBridge.productionRequest("{\"op\":\"stop\"}")
                runOnUiThread { if (!isDestroyed) finish() }
            }
        }, 6000)
    }

    private fun missing(): WebResourceResponse =
        WebResourceResponse("text/plain", "UTF-8", 404, "Not Found", emptyMap(),
            ByteArrayInputStream("Not found".toByteArray()))

    private fun isText(name: String) = name.substringAfterLast('.').lowercase() in
        setOf("html", "js", "mjs", "css", "json", "svg", "txt", "xml", "map")

    private fun mime(name: String): String = when (name.substringAfterLast('.').lowercase()) {
        "html" -> "text/html"
        "js", "mjs" -> "text/javascript"
        "css" -> "text/css"
        "json", "map" -> "application/json"
        "png" -> "image/png"
        "jpg", "jpeg" -> "image/jpeg"
        "webp" -> "image/webp"
        "svg" -> "image/svg+xml"
        "glb" -> "model/gltf-binary"
        "gltf" -> "model/gltf+json"
        "wasm" -> "application/wasm"
        "woff" -> "font/woff"
        "woff2" -> "font/woff2"
        "ttf" -> "font/ttf"
        "mp3" -> "audio/mpeg"
        "ogg" -> "audio/ogg"
        "mp4" -> "video/mp4"
        else -> "application/octet-stream"
    }

    companion object {
        private const val ASSET_HOST = "appassets.androidplatform.net"
        private const val GAME_URL = "https://$ASSET_HOST/game/index.html"
    }
}
