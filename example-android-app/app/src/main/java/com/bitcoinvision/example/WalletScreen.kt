package com.bitcoinvision.example

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowLeft
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxState
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import java.time.LocalDate
import java.time.format.DateTimeFormatter
import java.util.Locale

fun prettyDate(iso: String): String =
    LocalDate.parse(iso).format(DateTimeFormatter.ofPattern("d MMM yyyy", Locale.ENGLISH))

/** The one button under the key card, and where it goes. */
enum class Primary(val label: String, val tag: String) {
    Review("Review transaction", "review-waiting"),
    BackUp("Back up this wallet", "back-up"),
    Sign("Sign a transaction", "sign-transaction"),
    Scan("Scan phrase", "load-wallet-key"),
}

/** Pending review outranks everything; a fresh key asks to be backed up; then sign or scan. */
fun primaryFor(next: WalletAction): Primary = when (next) {
    WalletAction.Review -> Primary.Review
    WalletAction.BackUp -> Primary.BackUp
    WalletAction.Sign -> Primary.Sign
    WalletAction.LoadKey -> Primary.Scan
}

fun KeySource.provenance(): String = when (this) {
    KeySource.New -> "made on this phone"
    KeySource.WrittenPhrase -> "from written phrase"
    is KeySource.Strips -> "from strips ${minOf(first, second)} and ${maxOf(first, second)}"
}

fun backupLabel(checked: String?): String =
    checked?.let { "Backup checked ${prettyDate(it)}" } ?: "Backup not checked"

/** A swipe counts only when the card has travelled almost its whole width. */
const val UNLOAD_TRAVEL = 0.9f

/**
 * Whether a release at [offset] px (negative = leftward) on a card [width] px wide asks to unload.
 * Material's dismiss target is not consulted: a fast fling proposes the end value from a short
 * distance, and only the finger's travel may unload a key.
 */
fun unloadRequested(offset: Float, width: Int, loaded: Boolean): Boolean =
    loaded && width > 0 && -offset >= width * UNLOAD_TRAVEL

private fun SwipeToDismissBoxState.travel(): Float = runCatching { requireOffset() }.getOrDefault(0f)

/** The wallet page's only fixed explanatory line; the rest is state and refusals. */
const val BACKUP_WARNING_LINE = "Only on this phone until it is backed up."

@Composable
fun WalletScreen(
    wallets: WalletsViewModel,
    id: String,
    onBack: () -> Unit,
    onKeys: () -> Unit,
    onScan: () -> Unit,
    onStrips: () -> Unit,
    onConnect: () -> Unit,
    onBackUp: () -> Unit,
    onSign: () -> Unit,
    onReview: () -> Unit,
) {
    val wallet = wallets.wallet(id)
    if (wallet == null) {
        LaunchedEffect(Unit) { onBack() }
        return
    }
    LaunchedEffect(id) { wallets.leaveSigningUnless(id) }
    val key = wallets.key(id)
    val next = wallets.nextAction(id)
    val primary = primaryFor(next)
    var menu by remember(id) { mutableStateOf(false) }
    var renaming by remember(id) { mutableStateOf(false) }
    var removing by remember(id) { mutableStateOf(false) }
    var confirmUnload by remember(id) { mutableStateOf(false) }
    val requestUnload = {
        if (wallets.needsBackupWarning(id)) confirmUnload = true else wallets.unload(id)
    }

    WalletPage(
        wallet.name,
        onBack,
        titleTag = "wallet-name",
        actions = {
            IconButton(onClick = { menu = true }, modifier = Modifier.testTag("wallet-menu")) {
                Icon(Icons.Default.MoreVert, contentDescription = "Wallet options")
            }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                DropdownMenuItem(
                    text = { Text("Rename wallet") },
                    onClick = { menu = false; renaming = true },
                    modifier = Modifier.testTag("rename"),
                )
                DropdownMenuItem(
                    text = { Text("Remove wallet") },
                    onClick = { menu = false; removing = true },
                    modifier = Modifier.testTag("forget"),
                )
            }
        },
    ) {
        KeyCard(
            fingerprint = wallet.fingerprint,
            key = key,
            onUnload = requestUnload,
        )
        if (wallets.needsBackupWarning(id)) {
            Text(
                BACKUP_WARNING_LINE,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        wallets.notice?.takeIf { it.walletId == id }?.let { notice ->
            Text(
                notice.text,
                color = if (notice.refusal) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface,
                modifier = Modifier.testTag("notice"),
            )
        }
        if (next == WalletAction.Review) {
            Box(Modifier.size(0.dp).testTag("waiting"))
        }
        val go = when (primary) {
            Primary.Review -> onReview
            Primary.BackUp -> onBackUp
            Primary.Sign -> onSign
            Primary.Scan -> onScan
        }
        Column(Modifier.fillMaxWidth().animateContentSize(Motion.state())) {
            Button(onClick = go, modifier = Modifier.fillMaxWidth().testTag(primary.tag)) {
                AnimatedContent(
                    targetState = primary,
                    transitionSpec = { fadeIn(Motion.state()) togetherWith fadeOut(Motion.state()) },
                    label = "primary",
                ) { Text(it.label) }
            }
            if (primary == Primary.Scan) {
                TextButton(onClick = onStrips, modifier = Modifier.fillMaxWidth().testTag("wallet-scan-strips")) {
                    Text("Use share strips")
                }
            }
        }
        Card(Modifier.animateContentSize(Motion.state())) {
            if (primary != Primary.BackUp) {
                WalletLink(
                    "Back up on paper",
                    tag = "back-up",
                    onClick = onBackUp,
                    trailing = {
                        Text(
                            backupLabel(wallet.penlockBackup),
                            style = MaterialTheme.typography.labelMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.testTag("backup-status"),
                        )
                    },
                )
            }
            WalletLink("Connect to Sparrow", tag = "connect", onClick = onConnect)
            WalletLink("Key controls", tag = "key-controls", onClick = onKeys)
        }
        if (primary == Primary.BackUp) {
            // The row is gone while the button carries the action; the status still has a home.
            Text(
                backupLabel(wallet.penlockBackup),
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.testTag("backup-status"),
            )
        }
    }
    if (renaming) {
        RenameWalletDialog(wallet.name, onDismiss = { renaming = false }) { name -> wallets.rename(id, name) }
    }
    if (removing) {
        AlertDialog(
            onDismissRequest = { removing = false },
            title = { Text("Remove ${wallet.name}?") },
            text = {
                Text(
                    if (wallets.needsBackupWarning(id)) {
                        "This key has no paper backup yet. Removing it here removes it everywhere."
                    } else {
                        "Removes the saved wallet from this phone. Funds and paper backups are unaffected."
                    },
                )
            },
            confirmButton = {
                TextButton(
                    onClick = { wallets.forget(id); removing = false; onBack() },
                    modifier = Modifier.testTag("confirm-remove"),
                ) { Text("Remove wallet") }
            },
            dismissButton = {
                TextButton(onClick = { removing = false }, modifier = Modifier.testTag("cancel-remove")) { Text("Cancel") }
            },
        )
    }
    if (confirmUnload) {
        UnloadWarningDialog(
            onKeep = { confirmUnload = false },
            onUnload = { wallets.unload(id); confirmUnload = false },
        )
    }
}

/**
 * The page's one bold element: what the key is doing, in the worksheet's
 * face. Loaded, the card unloads by a full swipe to the left, or by its
 * accessibility action; released early it settles back and nothing happens.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun KeyCard(fingerprint: String, key: LoadedKey?, onUnload: () -> Unit) {
    val loaded = key != null
    // The dismiss state remembers its confirm lambda across recompositions, so it reads through
    // these rather than capturing a stale key or callback.
    val loadedNow by rememberUpdatedState(loaded)
    val unload by rememberUpdatedState(onUnload)
    var width by remember { mutableIntStateOf(0) }
    val haptics = LocalHapticFeedback.current
    lateinit var state: SwipeToDismissBoxState
    state = rememberSwipeToDismissBoxState(
        // Returning false keeps the card on the page; the release position alone decides.
        confirmValueChange = { value ->
            if (value == SwipeToDismissBoxValue.EndToStart && unloadRequested(state.travel(), width, loadedNow)) unload()
            false
        },
        positionalThreshold = { distance -> distance * UNLOAD_TRAVEL },
    )
    LaunchedEffect(state, width) {
        var armed = false
        snapshotFlow { state.travel() }.collect { offset ->
            val past = unloadRequested(offset, width, loadedNow)
            if (past && !armed) haptics.performHapticFeedback(HapticFeedbackType.LongPress)
            armed = past
        }
    }
    val container by animateColorAsState(
        if (loaded) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceContainer,
        animationSpec = Motion.state(), label = "key-card",
    )
    SwipeToDismissBox(
        state = state,
        enableDismissFromStartToEnd = false,
        enableDismissFromEndToStart = loaded,
        backgroundContent = {
            Row(
                Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceContainerHigh)
                    .padding(horizontal = 20.dp),
                horizontalArrangement = Arrangement.End,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("Unload", style = MaterialTheme.typography.labelLarge)
            }
        },
        modifier = Modifier
            .fillMaxWidth()
            .onSizeChanged { width = it.width }
            .testTag("key-card")
            .semantics {
                if (loaded) customActions = listOf(CustomAccessibilityAction("Unload key") { onUnload(); true })
            },
    ) {
        Card(colors = CardDefaults.cardColors(containerColor = container)) {
            AnimatedContent(
                targetState = key,
                transitionSpec = { fadeIn(Motion.state()) togetherWith fadeOut(Motion.state()) },
                label = "key-state",
            ) { current ->
                Column(Modifier.fillMaxWidth().padding(20.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        Box(
                            Modifier.size(10.dp).background(
                                if (current != null) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline,
                                CircleShape,
                            ),
                        )
                        Text(
                            if (current != null) "Key loaded" else "Key not loaded",
                            style = MaterialTheme.typography.headlineSmall.copy(fontFamily = PlexMono),
                            modifier = Modifier.testTag("key-status"),
                        )
                    }
                    Text(
                        fingerprint,
                        style = MaterialTheme.typography.titleLarge.copy(fontFamily = PlexMono),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.testTag("fingerprint"),
                    )
                    if (current != null) {
                        Text(
                            current.source.provenance(),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Icon(
                                Icons.AutoMirrored.Filled.KeyboardArrowLeft, contentDescription = null,
                                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            Text(
                                "swipe to unload",
                                style = MaterialTheme.typography.labelMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
fun UnloadWarningDialog(onKeep: () -> Unit, onUnload: () -> Unit) {
    AlertDialog(
        onDismissRequest = onKeep,
        title = { Text("Unload a key with no paper backup?") },
        text = { Text("You will need a backup you can recover from to load it again.") },
        confirmButton = {
            TextButton(onClick = onUnload, modifier = Modifier.testTag("confirm-unload")) { Text("Unload key") }
        },
        dismissButton = {
            TextButton(onClick = onKeep, modifier = Modifier.testTag("cancel-unload")) { Text("Keep key loaded") }
        },
    )
}

@Composable
private fun RenameWalletDialog(current: String, onDismiss: () -> Unit, onSave: (String) -> Boolean) {
    var name by remember { mutableStateOf(current) }
    val valid = normalizedWalletName(name) != null
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Rename wallet") },
        text = {
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                singleLine = true,
                label = { Text("Wallet name") },
                supportingText = { Text("1–40 characters") },
                isError = !valid,
                modifier = Modifier.testTag("wallet-name-input"),
            )
        },
        confirmButton = {
            TextButton(
                onClick = { if (onSave(name)) onDismiss() },
                enabled = valid,
                modifier = Modifier.testTag("save-name"),
            ) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
