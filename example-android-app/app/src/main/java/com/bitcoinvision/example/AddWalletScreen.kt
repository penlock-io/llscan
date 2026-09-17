package com.bitcoinvision.example

import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag

@Composable
fun AddWalletScreen(
    onNewKey: () -> Unit,
    onScanPhrase: () -> Unit,
    onScanStrips: () -> Unit,
    onBack: () -> Unit,
) {
    WalletPage("Add a wallet", onBack) {
        Text("Start fresh", style = MaterialTheme.typography.titleMedium)
        Button(onClick = onNewKey, modifier = Modifier.fillMaxWidth().testTag("new-key")) {
            Text("Create a new wallet")
        }
        Text("Already on paper", style = MaterialTheme.typography.titleMedium)
        Card {
            WalletLink("Scan a written phrase", tag = "scan-phrase", onClick = onScanPhrase)
            WalletLink("Scan two share strips", tag = "scan-strips", onClick = onScanStrips)
        }
    }
}
