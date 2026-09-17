package com.bitcoinvision.example

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import android.os.Build
import androidx.compose.runtime.mutableStateOf

class MainActivity : ComponentActivity() {
    /** A PSBT the system handed in, until the app takes it. */
    private val incoming = mutableStateOf<ByteArray?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        // The phrase must never reach screenshots or the recents view;
        // debug builds stay capturable so the emulator gates can
        // screenshot the flow.
        if (!BuildConfig.DEBUG) {
            window.setFlags(
                WindowManager.LayoutParams.FLAG_SECURE,
                WindowManager.LayoutParams.FLAG_SECURE,
            )
        }
        incoming.value = psbtIn(intent)
        setContent { VisionTheme { App(incoming) } }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        psbtIn(intent)?.let { incoming.value = it }
    }

    private fun psbtIn(intent: Intent?): ByteArray? =
        when (intent?.action) {
            Intent.ACTION_SEND ->
                intent.getStringExtra(Intent.EXTRA_TEXT)?.toByteArray()
                    ?: streamOf(intent)?.let(::read)
            Intent.ACTION_VIEW -> intent.data?.let(::read)
            else -> null
        }

    @Suppress("DEPRECATION")
    private fun streamOf(intent: Intent): Uri? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            intent.getParcelableExtra(Intent.EXTRA_STREAM)
        }

    private fun read(uri: Uri): ByteArray? =
        try {
            contentResolver.openInputStream(uri)?.use { it.readBytes() }
        } catch (e: java.io.IOException) {
            null
        } catch (e: SecurityException) {
            null
        }
}
