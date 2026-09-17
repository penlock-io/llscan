package com.bitcoinvision.example

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch

@Composable
private fun BackupStage(stage: Int) {
    val name = listOf("Write strips", "Check backup", "Store separately")[stage - 1]
    Text("Step $stage of 3 · $name", modifier = Modifier.testTag("backup-stage"))
    LinearProgressIndicator(progress = { stage / 3f }, modifier = Modifier.fillMaxWidth()
        .testTag("backup-progress").semantics { stateDescription = "Step $stage of 3 · $name" })
}

/** An invalidated attempt waits for an explicit exit, including during a pop animation. */
@Composable
fun EndedBackupScreen(onDone: () -> Unit) {
    BackHandler(onBack = onDone)
    TaskPage(
        title = "Penlock backup",
        onBack = onDone,
        actions = {
            Button(onClick = onDone, modifier = Modifier.fillMaxWidth().testTag("return-ended-backup")) {
                Text("Return")
            }
        },
    ) {
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text(
                "This backup attempt has ended",
                style = MaterialTheme.typography.headlineMedium,
                modifier = Modifier.testTag("backup-ended"),
            )
            Text("Its on-screen strips are no longer available. Return to your wallets to start again.")
        }
    }
}

/** Shares are made once on entry, never by changing pages or retrying a check. */
@Composable
fun SharesScreen(wallets: WalletsViewModel, onCheck: () -> Unit, onDone: () -> Unit) {
    val backup = wallets.backup
    if (backup == null) {
        EndedBackupScreen(onDone)
        return
    }
    val shares = backup.shares
    val pager = rememberPagerState { shares.size }
    val scope = rememberCoroutineScope()
    var confirmExit by remember { mutableStateOf(false) }
    val exit = { confirmExit = true }
    BackHandler(onBack = exit)
    TaskPage(
        title = "Penlock backup",
        onBack = exit,
        actions = {
            if (pager.currentPage < shares.lastIndex) {
                Button(
                    onClick = { scope.launch { pager.animateScrollToPage(pager.currentPage + 1) } },
                    modifier = Modifier.fillMaxWidth().testTag("next-share"),
                ) { Text("Next strip") }
            } else {
                Button(onClick = onCheck, modifier = Modifier.fillMaxWidth().testTag("check-strips")) {
                    Text("Check the strips")
                }
                TextButton(onClick = exit, modifier = Modifier.fillMaxWidth().testTag("done-shares")) {
                    Text("Finish without checking")
                }
            }
            TextButton(
                onClick = { scope.launch { pager.animateScrollToPage(pager.currentPage - 1) } },
                enabled = pager.currentPage > 0,
                modifier = Modifier.fillMaxWidth().testTag("prev-share"),
            ) { Text("Previous strip") }
        },
    ) {
        HorizontalPager(pager, Modifier.fillMaxSize()) { page ->
            Column(
                Modifier.fillMaxSize().testTag("strip-page-${page + 1}")
                    .verticalScroll(rememberScrollState()).padding(20.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                BackupStage(1)
                Text(
                    "Strip ${shares[page].number} of ${shares.size}",
                    style = MaterialTheme.typography.headlineMedium,
                    modifier = Modifier.testTag("share-title"),
                )
                Text("Copy each row onto the paper labelled Share ${shares[page].number}, in order.")
                OutlinedCard(Modifier.fillMaxWidth()) {
                    Column(
                        Modifier.testTag("strip-symbols").horizontalScroll(rememberScrollState()).padding(16.dp),
                        verticalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        shares[page].rows.forEachIndexed { i, row ->
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(
                                    "${i + 1}.",
                                    style = MaterialTheme.typography.titleMedium.copy(fontFamily = PlexMono),
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    modifier = Modifier.widthIn(min = 40.dp),
                                )
                                Text(
                                    row,
                                    style = MaterialTheme.typography.headlineMedium.copy(fontFamily = PlexMono),
                                    letterSpacing = 4.sp,
                                    softWrap = false,
                                    modifier = Modifier.testTag("share-row-${i + 1}")
                                        .semantics { contentDescription = "Row ${i + 1}: ${row.toList().joinToString(" ")}" },
                                )
                            }
                        }
                    }
                }
                Text(
                    if (page == shares.lastIndex) {
                        "Written all three? Photograph two strips to check that they recover this wallet."
                    } else {
                        "Keep these strips private. Any two can recover the wallet."
                    },
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text("This attempt is checked only after a matching pair is read back.")
            }
        }
    }
    if (confirmExit) {
        AlertDialog(
            onDismissRequest = { confirmExit = false },
            title = { Text("Leave this backup attempt?") },
            text = {
                Text(
                    "No new successful check will be recorded by leaving. Any earlier check date stays " +
                        "unchanged. The strips on this screen will be discarded; starting again makes " +
                        "a new set. Don’t mix strips from different attempts. Your key stays loaded.",
                )
            },
            confirmButton = {
                TextButton(onClick = onDone, modifier = Modifier.testTag("confirm-exit-backup")) {
                    Text("Exit backup")
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmExit = false }, modifier = Modifier.testTag("keep-writing")) {
                    Text("Keep writing")
                }
            },
        )
    }
}

@Composable
fun BackupCheckScreen(onScan: () -> Unit, onBack: () -> Unit) {
    BackHandler(onBack = onBack)
    TaskPage(
        title = "Check backup",
        onBack = onBack,
        actions = {
            Button(onClick = onScan, modifier = Modifier.fillMaxWidth().testTag("scan-backup")) {
                Text("Scan two strips")
            }
        },
    ) {
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            BackupStage(2)
            Text("Check what you wrote", style = MaterialTheme.typography.headlineMedium)
            Text("Choose two different strips from this set. Photograph one, then the other.")
            Text(
                "The recovered words are compared with this wallet’s loaded key. " +
                    "Only a matching pair records a successful check.",
            )
            Text(
                "The third strip is not verified by this check. Keep all three private.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

/** Only this attempt's check, never a saved date, selects the success state. */
@Composable
fun CheckedScreen(wallets: WalletsViewModel, onAgain: () -> Unit, onRetry: () -> Unit, onDone: () -> Unit) {
    val backup = wallets.backup
    val key = backup?.let { wallets.key(it.walletId) }
    val check = backup?.check
    if (key == null || check == null) {
        EndedBackupScreen(onDone)
        return
    }
    val mismatch = check.mismatch
    val success = mismatch == null
    val back = if (success) onDone else onAgain
    BackHandler(onBack = back)
    TaskPage(
        title = if (success) "Backup checked" else "Check needs attention",
        onBack = back,
        actions = {
            if (success) {
                Button(onClick = onDone, modifier = Modifier.fillMaxWidth().testTag("done-check")) {
                    Text("Return to wallet")
                }
            } else {
                Button(onClick = onRetry, modifier = Modifier.fillMaxWidth().testTag("retry-check")) {
                    Text("Scan the pair again")
                }
                OutlinedButton(onClick = onAgain, modifier = Modifier.fillMaxWidth().testTag("back-to-shares")) {
                    Text("Review the written strips")
                }
            }
        },
    ) {
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            BackupStage(if (success) 3 else 2)
            Card(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text(
                        if (success) "Two strips recover this wallet" else "The strips differ",
                        style = MaterialTheme.typography.headlineSmall,
                    )
                    Text(
                        if (success) {
                            "The pair you scanned matches the loaded key, word for word. " +
                                "The third strip has not been checked."
                        } else {
                            val row = mismatch!!
                            val read = check.read.getOrNull(row) ?: "nothing"
                            val expected = key.words.getOrNull(row) ?: "nothing"
                            "Row ${row + 1} differs: the strips give \"$read\", the key has \"$expected\". " +
                                "Compare that row on both strips with the on-screen symbols, rewrite, then scan again."
                        },
                        modifier = Modifier.testTag("check-result"),
                    )
                }
            }
            Text(
                if (success) "Store all three strips separately" else "This attempt has not passed",
                style = MaterialTheme.typography.titleLarge,
            )
            Text(
                if (success) "Keep each strip in a different safe place. Any two can recover the wallet."
                else "No successful check was recorded for this attempt. Any earlier check date is unchanged.",
            )
        }
    }
}
