package com.bitcoinvision.example

import android.content.Context
import android.content.Intent
import android.util.Base64
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import uniffi.bitcoin_vision_mobile.Review
import uniffi.bitcoin_vision_mobile.psbtQrParts
import java.text.NumberFormat
import java.util.Locale

/** Satoshis as the pages print them. */
fun sats(n: ULong): String = NumberFormat.getIntegerInstance(Locale.ENGLISH).format(n.toLong()) + " sats"

/** Presentation only: validated outgoing outputs plus fee, never the input total. */
fun totalLeaving(amounts: List<ULong>, fee: ULong): ULong = amounts.fold(fee) { sum, amount -> sum + amount }

/** Payload bytes per animated frame: what frostsnap shows Sparrow, comfortably read at arm's length. */
const val QR_FRAGMENT_BYTES = 400u

/** The PSBT to sign, off Sparrow's screen or from the phone's files. */
@Composable
fun SignPickScreen(
    wallets: WalletsViewModel,
    onScanQr: () -> Unit,
    onReviewed: () -> Unit,
    onBack: () -> Unit,
    fixtureRoute: Boolean = BuildConfig.DEBUG && onEmulator,
) {
    val context = LocalContext.current
    val take = { bytes: ByteArray ->
        wallets.receive(bytes)
        onReviewed()
    }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        uri?.let { context.contentResolver.openInputStream(it)?.use { s -> s.readBytes() } }?.let(take)
    }
    BackHandler(onBack = onBack)
    TaskPage("Sign a transaction", onBack, actions = {
        Button(onClick = onScanQr, modifier = Modifier.fillMaxWidth().testTag("scan-qr")) { Text("Scan QR code") }
        OutlinedButton(
            onClick = { picker.launch(arrayOf("*/*")) },
            modifier = Modifier.fillMaxWidth().testTag("choose-file"),
        ) { Text("Choose transaction file") }
    }) {
        SigningBody {
            Text("Show Sparrow's QR code to the camera.", style = MaterialTheme.typography.titleLarge)
            Text("Review first, then sign. Nothing is broadcast.", color = MaterialTheme.colorScheme.onSurfaceVariant)
            // The directory this reads is one only adb can fill, so it is
            // of no use on a phone; the click-throughs run on the emulator.
            if (fixtureRoute) {
                var noFixture by remember { mutableStateOf(false) }
                TextButton(
                    onClick = {
                        val psbt = latestPsbt(context)
                        noFixture = psbt == null
                        psbt?.let(take)
                    },
                    modifier = Modifier.testTag("from-file"),
                ) { Text("From file") }
                if (noFixture) {
                    Text(
                        "No transaction in the app's files/psbts directory.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.testTag("from-file-empty"),
                    )
                }
            }
        }
    }
}

private fun latestPsbt(context: Context): ByteArray? =
    context.getExternalFilesDir("psbts")
        ?.listFiles()
        ?.filter { it.isFile && it.name != "signed.psbt" }
        ?.maxByOrNull { it.lastModified() }
        ?.readBytes()

/** Back keeps a pending request; Dismiss explicitly discards it. */
@Composable
fun ReviewPsbtScreen(
    wallets: WalletsViewModel,
    onSigned: () -> Unit,
    onLoadKey: (String) -> Unit,
    onChooseAnother: () -> Unit,
    onBack: () -> Unit,
    onDone: () -> Unit,
) {
    when (val signing = wallets.signing) {
        null -> EndedSigning(onDone)
        // Keep the reviewed summary on the outgoing page during the signing transition.
        is Signing.Signed -> TaskPage("Review transaction", onDone, actions = {}) {
            SigningBody {
                SigningIdentity(wallets.wallet(signing.walletId)?.name, signing.review)
                PaymentSummary(signing.review)
            }
        }
        is Signing.Refused -> {
            BackHandler(onBack = onDone)
            TaskPage("Not signed", onDone, actions = {
                Button(onClick = onChooseAnother, modifier = Modifier.fillMaxWidth().testTag("choose-another")) {
                    Text("Choose another file")
                }
                TextButton(onClick = onDone, modifier = Modifier.fillMaxWidth().testTag("done-signing")) {
                    Text("Return")
                }
            }) {
                SigningBody {
                    Text(
                        signing.reason, style = MaterialTheme.typography.titleMedium,
                        color = MaterialTheme.colorScheme.error, modifier = Modifier.testTag("refusal"),
                    )
                    Text("Nothing was signed.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
        is Signing.Pending -> key(signing) {
            val loaded = wallets.key(signing.walletId) != null
            BackHandler(onBack = onBack)
            TaskPage("Review transaction", onBack, actions = {
                if (loaded) {
                    Button(
                        // Capture the request drawn, not whatever is current when the tap lands.
                        onClick = { if (wallets.sign(signing)) onSigned() },
                        modifier = Modifier.fillMaxWidth().testTag("sign"),
                    ) { Text("Sign transaction") }
                } else {
                    Text(
                        "Load the key, then sign.",
                        style = MaterialTheme.typography.bodyMedium, modifier = Modifier.testTag("key-missing"),
                    )
                    Button(
                        onClick = { onLoadKey(signing.walletId) },
                        modifier = Modifier.fillMaxWidth().testTag("load-key"),
                    ) { Text("Load key") }
                }
                TextButton(onClick = onDone, modifier = Modifier.fillMaxWidth().testTag("dismiss")) {
                    Text("Dismiss transaction")
                }
            }) {
                SigningBody {
                    SigningIdentity(wallets.wallet(signing.walletId)?.name, signing.review)
                    PaymentSummary(signing.review)
                }
            }
        }
    }
}

@Composable
private fun EndedSigning(onDone: () -> Unit) {
    BackHandler(onBack = onDone)
    TaskPage("Transaction", onDone, actions = {
        Button(onClick = onDone, modifier = Modifier.fillMaxWidth().testTag("done-signing")) { Text("Return") }
    }) {
        SigningBody { Text("Nothing to sign", modifier = Modifier.testTag("signing-ended")) }
    }
}

@Composable
private fun SigningBody(content: @Composable ColumnScope.() -> Unit) {
    Column(
        Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp), content = content,
    )
}

@Composable
private fun SigningIdentity(name: String?, review: Review) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(name ?: "Wallet", style = MaterialTheme.typography.titleLarge, modifier = Modifier.testTag("sign-title"))
        Text(
            "Fingerprint ${review.fingerprint()}",
            style = MaterialTheme.typography.bodyMedium.copy(fontFamily = PlexMono),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun PaymentSummary(review: Review) {
    val leaving = review.leaving()
    var details by remember(review) { mutableStateOf(false) }
    Text("To recipients", style = MaterialTheme.typography.titleMedium)
    if (leaving.isEmpty()) {
        Text("No recipient payments. Only the fee leaves this wallet.", modifier = Modifier.testTag("leaving-none"))
    }
    leaving.forEachIndexed { i, line ->
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Recipient ${i + 1}", style = MaterialTheme.typography.labelLarge)
                Text(
                    sats(line.amount), style = MaterialTheme.typography.titleLarge.copy(fontFamily = PlexMono),
                    modifier = Modifier.testTag("leaving-${i + 1}-amount"),
                )
                SelectionContainer {
                    Text(
                        line.address, style = MaterialTheme.typography.bodyMedium.copy(fontFamily = PlexMono),
                        modifier = Modifier.testTag("leaving-${i + 1}-address"),
                    )
                }
            }
        }
    }
    AmountLine("Network fee", sats(review.fee()), "fee")
    HorizontalDivider()
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text("Total leaving this wallet", style = MaterialTheme.typography.titleMedium)
        Text(
            sats(totalLeaving(leaving.map { it.amount }, review.fee())),
            style = MaterialTheme.typography.headlineMedium.copy(fontFamily = PlexMono),
            modifier = Modifier.testTag("total-leaving"),
        )
        Text(
            "Recipient payments + network fee", style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
    TextButton(
        onClick = { details = !details },
        modifier = Modifier.fillMaxWidth().testTag("transaction-details")
            .semantics { stateDescription = if (details) "Expanded" else "Collapsed" },
    ) { Text(if (details) "Hide transaction details" else "Transaction details") }
    if (details) {
        AmountLine("Change back to this wallet", sats(review.change()), "change")
        AmountLine("Input total from this wallet", sats(review.inputsTotal()), "inputs-total")
    }
}

/** Stacked values keep large amounts readable at increased font sizes. */
@Composable
private fun AmountLine(label: String, value: String, tag: String) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(label, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value, style = MaterialTheme.typography.titleMedium.copy(fontFamily = PlexMono), modifier = Modifier.testTag(tag))
    }
}

/** Signed locally: export is a hand-off, never a broadcast-success claim. */
@Composable
fun SignedScreen(wallets: WalletsViewModel, onDone: () -> Unit) {
    val signed = wallets.signing as? Signing.Signed
    if (signed == null) {
        EndedSigning(onDone)
        return
    }
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val text = Base64.encodeToString(signed.bytes, Base64.NO_WRAP)
    val encoder = remember(signed.bytes) { runCatching { psbtQrParts(signed.bytes, QR_FRAGMENT_BYTES) }.getOrNull() }
    BackHandler(onBack = onDone)
    TaskPage("Signed transaction", onDone, snackbar = snackbar, actions = {
        Button(
            onClick = {
                val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text)
                context.startActivity(Intent.createChooser(send, "Signed transaction"))
            },
            modifier = Modifier.fillMaxWidth().testTag("share-signed"),
        ) { Text("Share signed transaction") }
        OutlinedButton(
            onClick = {
                clipboard.setText(AnnotatedString(text))
                scope.launch { snackbar.showSnackbar("Signed transaction copied") }
            },
            modifier = Modifier.fillMaxWidth().testTag("copy-signed"),
        ) { Text("Copy") }
        TextButton(onClick = onDone, modifier = Modifier.fillMaxWidth().testTag("done-signed")) { Text("Done") }
    }) {
        SigningBody {
            encoder?.let {
                AnimatedQr(it, "Signed transaction as a QR code", Modifier.fillMaxWidth().testTag("signed-qr"))
            }
            Text("Signed — not broadcast", style = MaterialTheme.typography.titleLarge, modifier = Modifier.testTag("signed"))
            Text("Scan it with Sparrow, or hand the file back, to broadcast.", color = MaterialTheme.colorScheme.onSurfaceVariant)
            SigningIdentity(wallets.wallet(signed.walletId)?.name, signed.review)
            PaymentSummary(signed.review)
        }
    }
}
