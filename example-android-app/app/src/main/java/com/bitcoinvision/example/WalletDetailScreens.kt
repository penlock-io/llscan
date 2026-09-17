package com.bitcoinvision.example

import android.content.Intent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch

fun KeySource.description(): String = when (this) {
    KeySource.New -> "Made on this phone."
    KeySource.WrittenPhrase -> "Loaded from your written phrase."
    is KeySource.Strips ->
        "Loaded from share strips ${minOf(first, second)} and ${maxOf(first, second)}."
}

@Composable
fun ConnectWalletScreen(wallets: WalletsViewModel, id: String, onBack: () -> Unit) {
    val wallet = wallets.wallet(id)
    if (wallet == null) {
        LaunchedEffect(Unit) { onBack() }
        return
    }
    val clipboard = LocalClipboardManager.current
    val context = LocalContext.current
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    WalletPage("Connect to Sparrow", onBack, snackbar = snackbar) {
        Text(wallet.name, style = MaterialTheme.typography.titleLarge)
        Text(
            "Scan this in Sparrow to watch the wallet and build transactions for this phone to sign.",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        StaticQr(wallet.descriptor, "Descriptor of ${wallet.name} as a QR code", "descriptor-qr", Modifier.fillMaxWidth())
        Card {
            SelectionContainer {
                Text(
                    wallet.descriptor,
                    style = MaterialTheme.typography.bodyMedium.copy(fontFamily = PlexMono),
                    modifier = Modifier.padding(16.dp).testTag("descriptor"),
                )
            }
        }
        Button(
            onClick = {
                val send = Intent(Intent.ACTION_SEND)
                    .setType("text/plain")
                    .putExtra(Intent.EXTRA_TEXT, wallet.descriptor)
                context.startActivity(Intent.createChooser(send, "Descriptor of ${wallet.name}"))
            },
            modifier = Modifier.fillMaxWidth().testTag("share-descriptor"),
        ) { Text("Share descriptor") }
        OutlinedButton(
            onClick = {
                clipboard.setText(AnnotatedString(wallet.descriptor))
                scope.launch { snackbar.showSnackbar("Descriptor copied") }
            },
            modifier = Modifier.fillMaxWidth().testTag("copy-descriptor"),
        ) { Text("Copy descriptor") }
    }
}

@Composable
fun KeyControlsScreen(
    wallets: WalletsViewModel,
    id: String,
    onBack: () -> Unit,
    onLoadKey: () -> Unit,
) {
    val wallet = wallets.wallet(id)
    if (wallet == null) {
        LaunchedEffect(Unit) { onBack() }
        return
    }
    val key = wallets.key(id)
    var confirmUnload by remember(id) { mutableStateOf(false) }
    // Revealing is local to this visit, never restored from saved state.
    DisposableEffect(id) {
        onDispose {
            if (wallets.revealed == id) wallets.revealed = null
        }
    }
    WalletPage("Key controls", onBack) {
        Text(wallet.name, style = MaterialTheme.typography.titleLarge)
        Text(wallet.fingerprint, style = MaterialTheme.typography.bodyMedium.copy(fontFamily = PlexMono))
        Text(
            if (key == null) "Key not loaded" else "Key loaded",
            style = MaterialTheme.typography.titleMedium,
            modifier = Modifier.testTag("key-status"),
        )
        if (key == null) {
            Button(onClick = onLoadKey, modifier = Modifier.fillMaxWidth().testTag("load-controls-key")) {
                Text("Load key")
            }
        } else {
            Text(key.source.description(), color = MaterialTheme.colorScheme.onSurfaceVariant)
            if (wallets.needsBackupWarning(id)) {
                Text("No paper backup yet.", color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            val shown = wallets.revealed == id
            OutlinedButton(
                onClick = { wallets.revealed = if (shown) null else id },
                modifier = Modifier.fillMaxWidth().testTag("show-phrase"),
            ) { Text(if (shown) "Hide the phrase" else "Show the phrase") }
            if (shown) {
                Text("Keep the screen private.", style = MaterialTheme.typography.labelLarge)
                PhraseBody(key.words)
            }
            TextButton(
                onClick = { if (wallets.needsBackupWarning(id)) confirmUnload = true else wallets.unload(id) },
                modifier = Modifier.fillMaxWidth().testTag("unload"),
            ) { Text("Unload the key") }
        }
    }
    if (confirmUnload) {
        UnloadWarningDialog(
            onKeep = { confirmUnload = false },
            onUnload = { wallets.unload(id); confirmUnload = false },
        )
    }
}

@Composable
fun PhraseBody(words: List<String>) {
    val half = (words.size + 1) / 2
    Row(Modifier.padding(vertical = 4.dp)) {
        for (column in 0..1) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                for (i in column * half until minOf((column + 1) * half, words.size)) {
                    Text(
                        "${i + 1}. ${words[i]}",
                        style = MaterialTheme.typography.titleLarge.copy(fontFamily = PlexMono),
                        modifier = Modifier.testTag("word-${i + 1}"),
                    )
                }
            }
        }
    }
}
