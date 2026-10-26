package dev.holochain.turbokart.mobile

import android.app.Activity
import android.graphics.Color
import android.os.Bundle
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

        val view = WebView(this)
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
        view.webChromeClient = WebChromeClient()
        view.webViewClient = object : WebViewClient() {
            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                // Never navigate to arbitrary external pages from the game screen.
                return request.url.scheme != "https" || request.url.host != ASSET_HOST
            }

            override fun shouldInterceptRequest(view: WebView?, request: WebResourceRequest?): WebResourceResponse? {
                val url = request?.url ?: return null
                if (url.scheme != "https" || url.host != ASSET_HOST) return null
                val path = url.path ?: return missing()
                if (!path.startsWith("/game/")) return missing()
                val relative = path.removePrefix("/game/")
                if (relative.isEmpty() || relative.split('/').any { it == ".." || it == "." }) return missing()
                return try {
                    WebResourceResponse(mime(relative), if (isText(relative)) "UTF-8" else null,
                        assets.open("game/$relative"))
                } catch (_: Exception) { missing() }
            }
        }
        game = view
        setContentView(view)
        view.loadUrl("https://$ASSET_HOST/game/index.html")
    }

    override fun onPause() { game?.onPause(); super.onPause() }
    override fun onResume() { super.onResume(); game?.onResume() }
    override fun onDestroy() { game?.destroy(); game = null; super.onDestroy() }

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

    companion object { private const val ASSET_HOST = "appassets.androidplatform.net" }
}
