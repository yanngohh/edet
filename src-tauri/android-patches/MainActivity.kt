package org.edet

import android.os.Bundle
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import android.webkit.WebChromeClient
import android.webkit.ConsoleMessage
import android.util.Log
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    WebView.setWebContentsDebuggingEnabled(true)
    super.onCreate(savedInstanceState)

    val root = window.decorView.findViewById<ViewGroup>(android.R.id.content)
    
    // 1. Try to find and set up immediately
    setupWebViewLogger(root)

    // 2. Register listener to catch it as soon as Tauri dynamically attaches it
    root.setOnHierarchyChangeListener(object : ViewGroup.OnHierarchyChangeListener {
      override fun onChildViewAdded(parent: View?, child: View?) {
        setupWebViewLogger(root)
      }
      override fun onChildViewRemoved(parent: View?, child: View?) {}
    })
  }

  private fun setupWebViewLogger(root: ViewGroup) {
    findWebView(root)?.let { webView ->
      // Prevent double-binding if we already wrapped it
      if (webView.tag == "logged") return
      webView.tag = "logged"
      
      val oldClient = webView.webChromeClient
      webView.webChromeClient = object : WebChromeClient() {
        override fun onConsoleMessage(consoleMessage: ConsoleMessage): Boolean {
          val msg = "[JS Console] ${consoleMessage.message()} (${consoleMessage.sourceId()}:${consoleMessage.lineNumber()})"
          if (consoleMessage.messageLevel() == ConsoleMessage.MessageLevel.ERROR) {
            Log.e("TauriWebConsole", msg)
          } else if (consoleMessage.messageLevel() == ConsoleMessage.MessageLevel.WARNING) {
            Log.w("TauriWebConsole", msg)
          } else {
            Log.i("TauriWebConsole", msg)
          }
          return oldClient?.onConsoleMessage(consoleMessage) ?: true
        }
      }
      Log.i("TauriWebConsole", "Successfully attached console logger to WebView")
    }
  }

  private fun findWebView(view: View): WebView? {
    if (view is WebView) {
      return view
    }
    if (view is ViewGroup) {
      for (i in 0 until view.childCount) {
        val result = findWebView(view.getChildAt(i))
        if (result != null) return result
      }
    }
    return null
  }
}
