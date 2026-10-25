package dev.holochain.turbokart.mobile

import android.app.Activity
import android.content.pm.ActivityInfo
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.webkit.ConsoleMessage
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import android.webkit.RenderProcessGoneDetail
import android.widget.TextView
import androidx.webkit.WebViewAssetLoader

/**
 * Standalone single-player gameplay preview using Turbo Kart's existing
 * Three.js files and driving physics. Native Holochain and Iroh endpoints
 * are NOT exposed to this WebView. Multiplayer remains on MainActivity.
 */
class TouchGameActivity : Activity() {
    private var gameWebView: WebView? = null
    private val gameUrl = "https://appassets.androidplatform.net/assets/game/index.html"

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        enterImmersiveMode()

        // The game files are staged by scripts/build-turbo-kart-touch-apk.sh;
        // don't crash if this is the ordinary test APK without these assets.
        val packaged = runCatching {
            assets.open("game/index.html").use { it.read() }
            assets.open("mobile/touch-controls.js").use { it.read() }
        }.isSuccess
        if (!packaged) {
            showError(
                "Game files are not packaged in this APK.\n\n" +
                "Build the touch preview with:\n" +
                "bash scripts/build-turbo-kart-touch-apk.sh\n\n" +
                "The existing multiplayer test screen is unchanged."
            )
            return
        }

        val loader = WebViewAssetLoader.Builder()
            .setDomain("appassets.androidplatform.net")
            .addPathHandler("/assets/", WebViewAssetLoader.AssetsPathHandler(this))
            .build()

        val web = WebView(this)
        gameWebView = web
        web.setBackgroundColor(Color.rgb(11, 16, 32))
        web.isLongClickable = false
        web.setOnLongClickListener { true }

        web.settings.apply {
            javaScriptEnabled = true
            domStorageEnabled = true
            allowFileAccess = false
            allowContentAccess = false
            javaScriptCanOpenWindowsAutomatically = false
            setSupportMultipleWindows(false)
            builtInZoomControls = false
            displayZoomControls = false
            loadsImagesAutomatically = true
            mediaPlaybackRequiresUserGesture = true
            cacheMode = WebSettings.LOAD_DEFAULT
            mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
        }

        // No addJavascriptInterface, Holochain token, admin connection, or
        // injected signing capability. This page is gameplay-only.
        web.webChromeClient = object : WebChromeClient() {
            override fun onConsoleMessage(message: ConsoleMessage): Boolean {
                Log.d("TurboKartTouch", "${message.message()} @ ${message.sourceId()}:${message.lineNumber()}")
                return true
            }
        }
        web.webViewClient = object : WebViewClient() {
            override fun shouldInterceptRequest(
                view: WebView,
                request: WebResourceRequest
            ): WebResourceResponse? = loader.shouldInterceptRequest(request.url)

            override fun shouldOverrideUrlLoading(
                view: WebView,
                request: WebResourceRequest
            ): Boolean {
                // Do not navigate this preview to arbitrary websites.
                return request.url.scheme != "https" ||
                    request.url.host != "appassets.androidplatform.net" ||
                    !request.url.path.orEmpty().startsWith("/assets/game/")
            }

            override fun onPageFinished(view: WebView, url: String) {
                if (url != gameUrl) return
                val script = runCatching {
                    assets.open("mobile/touch-controls.js")
                        .bufferedReader(Charsets.UTF_8).use { it.readText() }
                }.getOrElse {
                    Log.e("TurboKartTouch", "Touch script missing", it)
                    return
                }
                // Touch controls are dispatched as the exact KeyboardEvent
                // codes consumed by the existing Turbo Kart InputController.
                view.evaluateJavascript(script, null)
            }

            override fun onRenderProcessGone(
                view: WebView,
                detail: RenderProcessGoneDetail
            ): Boolean {
                Log.e("TurboKartTouch", "WebView renderer exited, crashed=${detail.didCrash()}")
                gameWebView = null
                view.destroy()
                showError("The 3D renderer stopped. Reopen Touch Preview to retry.\n\n" +
                    "No Holochain player data has been changed.")
                return true
            }
        }
        setContentView(web)
        web.loadUrl(gameUrl)
    }

    private fun showError(text: String) {
        setContentView(TextView(this).apply {
            setTextColor(Color.WHITE)
            setBackgroundColor(Color.rgb(11, 16, 32))
            textSize = 18f
            setPadding(30, 60, 30, 30)
            this.text = text
        })
    }

    private fun enterImmersiveMode() {
        if (Build.VERSION.SDK_INT >= 30) {
            window.insetsController?.let { controller ->
                controller.hide(WindowInsets.Type.statusBars() or WindowInsets.Type.navigationBars())
                controller.systemBarsBehavior =
                    WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            }
        } else {
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility =
                View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY or
                View.SYSTEM_UI_FLAG_FULLSCREEN or
                View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE or
                View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or
                View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) enterImmersiveMode()
    }

    override fun onPause() {
        // Release held throttle/steering/drift when switching apps.
        gameWebView?.evaluateJavascript("window.TurboKartTouch?.releaseAll?.()", null)
        gameWebView?.onPause()
        super.onPause()
    }

    override fun onResume() {
        super.onResume()
        enterImmersiveMode()
        gameWebView?.onResume()
    }

    override fun onDestroy() {
        gameWebView?.let { web ->
            web.evaluateJavascript("window.TurboKartTouch?.dispose?.()", null)
            web.stopLoading()
            web.destroy()
        }
        gameWebView = null
        super.onDestroy()
    }
}
