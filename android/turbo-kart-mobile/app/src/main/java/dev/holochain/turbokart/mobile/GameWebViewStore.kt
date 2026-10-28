package dev.holochain.turbokart.mobile

import android.app.Activity
import android.content.MutableContextWrapper
import android.util.Log
import android.view.ViewGroup
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebView
import android.webkit.WebViewClient

/** One reusable renderer; no Activity, message capability or game document is retained. */
internal object GameWebViewStore {
    private var idle: WebView? = null
    fun obtain(activity: Activity): WebView {
        val view = idle ?: WebView(MutableContextWrapper(activity))
        idle = null
        (view.context as MutableContextWrapper).baseContext = activity
        return view
    }
    fun release(view: WebView, activity: Activity) {
        (view.parent as? ViewGroup)?.removeView(view)
        view.stopLoading()
        view.webChromeClient = null
        view.webViewClient = object : WebViewClient() {
            override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
                Log.w("TurboKartWeb", "Idle renderer reclaimed; didCrash=${detail.didCrash()}")
                if (idle === view) idle = null
                view.destroy()
                return true
            }
        }
        // Navigation drops the old page, GPU scene and all JavaScript timers.
        // Keep the empty renderer alive instead of destroying it on every Back.
        view.loadUrl("about:blank")
        view.onPause()
        (view.context as MutableContextWrapper).baseContext = activity.applicationContext
        idle?.destroy()
        idle = view
    }
}
