package com.bitcoinvision.example

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag

/** User-operated camera entry; compiled only into the isolated debug identity. */
class CaptureDiagnosticActivity : ComponentActivity() {
    private val sheet: SheetViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            VisionTheme {
                var started by rememberSaveable { mutableStateOf(false) }
                if (!started) {
                    TaskPage(
                        title = "Capture framing diagnostic",
                        onBack = { finish() },
                        actions = {
                            Button(onClick = {
                                sheet.start()
                                started = true
                            }, modifier = Modifier.testTag("start-capture-diagnostic")) {
                                Text("Open camera")
                            }
                        },
                    ) {
                        Text("Use a public test page, never wallet recovery words. " +
                            "Take one photo for the agreed before/after check, then save its raw photo and metadata. " +
                            "This diagnostic cannot create a wallet or sign a transaction.")
                    }
                } else if (sheet.stage == SheetStage.Reviewing) {
                    ReviewScreen(sheet, action = "Close diagnostic", onAccept = { finish() }, onBack = { finish() })
                } else {
                    SheetScanScreen(sheet, onReviewed = {}, onExit = { finish() })
                }
            }
        }
    }
}
