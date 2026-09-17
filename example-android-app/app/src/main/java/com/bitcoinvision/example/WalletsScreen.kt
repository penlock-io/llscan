package com.bitcoinvision.example

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.AccountBalanceWallet
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp

/** The one place the app states its safety promise, so no other screen has to. */
const val SAFETY_LINE = "Keys are held in memory only. This phone never saves recovery words."

@Composable
fun WalletsScreen(wallets: WalletsViewModel, onAdd: () -> Unit, onOpen: (String) -> Unit) {
    WalletPage("Wallets", floatingAction = {
        if (wallets.wallets.isNotEmpty()) ExtendedFloatingActionButton(
            onClick = onAdd,
            modifier = Modifier.testTag("add-wallet"),
            icon = { Icon(Icons.Default.Add, contentDescription = null) },
            text = { Text("Add wallet") },
        )
    }) {
        if (wallets.wallets.isEmpty()) {
            Text("Your wallet, on paper", style = MaterialTheme.typography.headlineSmall)
            Text("Create one, or scan one from paper.", modifier = Modifier.testTag("empty"))
            Button(onClick = onAdd, modifier = Modifier.testTag("add-wallet")) { Text("Add a wallet") }
        } else {
            Card {
                wallets.wallets.forEachIndexed { i, wallet ->
                    val loaded = wallets.key(wallet.id) != null
                    ListItem(
                        headlineContent = { Text(wallet.name) },
                        supportingContent = {
                            Text(
                                "${wallet.fingerprint} · ${if (loaded) "key loaded" else "key not loaded"}",
                                style = MaterialTheme.typography.bodyMedium.copy(fontFamily = PlexMono),
                                modifier = Modifier.testTag("wallet-$i-status"),
                            )
                        },
                        leadingContent = { Icon(Icons.Default.AccountBalanceWallet, contentDescription = null) },
                        modifier = Modifier.clickable { onOpen(wallet.id) }.testTag("wallet-$i"),
                    )
                }
            }
            Spacer(Modifier.height(80.dp))
        }
        Text(SAFETY_LINE, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}
