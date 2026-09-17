package com.bitcoinvision.example

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableState
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.NavBackStackEntry
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController

// The scan flows serve three purposes, carried in their routes: a
// new wallet, the key of a saved wallet (its id), or a backup check.
private const val ADD = "add"
private const val CHECK = "check"

private fun NavBackStackEntry.purpose(): String = arguments?.getString("for") ?: ADD
private fun NavBackStackEntry.task(): String = arguments?.getString("task") ?: "wallet"
private fun loadRoute(id: String, task: String) = "load/$id/$task"
private fun scanRoute(screen: String, purpose: String, task: String = "wallet") = "$screen/$purpose/$task"

@Composable
fun App(
    incoming: MutableState<ByteArray?>,
    wallets: WalletsViewModel = viewModel(),
    sheet: SheetViewModel = viewModel(),
    strips: StripsViewModel = viewModel(),
    creation: NewSeedViewModel = viewModel(),
) {
    val nav = rememberNavController()
    // Replace the entire signing visit, including nested key loading, while
    // keeping the original caller beneath it rather than obsolete review pages.
    val reviewIncoming = {
        val route = nav.currentBackStack.value.firstOrNull {
            it.destination.route in setOf("sign/pick", "sign/scan", "sign/review", "sign/signed")
        }?.destination?.route
        if (nav.currentBackStack.value.any { it.arguments?.getString("task") == "sign" }) {
            sheet.forget()
            strips.cancel()
        }
        nav.navigate("sign/review") {
            if (route != null) popUpTo(route) { inclusive = true }
            launchSingleTop = true
        }
    }
    // A PSBT shared to the app, from wherever the person was.
    LaunchedEffect(incoming.value) {
        val bytes = incoming.value ?: return@LaunchedEffect
        incoming.value = null
        creation.cancel()
        wallets.receive(bytes)
        reviewIncoming()
    }
    // The review sits on top of wherever the PSBT found the person:
    // done with it, they are back there.
    val leaveSigning = {
        wallets.dismissSigning()
        nav.popBackStack()
    }
    val open = { id: String ->
        // Creation may immediately open the write-down page before the overview composes.
        wallets.leaveSigningUnless(id)
        nav.navigate("wallet/$id") { popUpTo("wallets") }
    }
    // The wallet page opens the camera directly; other callers go through the
    // method chooser. Leaving a scan returns to whichever of the two sent it.
    val viaChooser = { purpose: String, task: String ->
        nav.currentBackStack.value.any {
            it.destination.route == "load/{id}/{task}" &&
                it.arguments?.getString("id") == purpose && it.arguments?.getString("task") == task
        }
    }
    val leave = { purpose: String, task: String ->
        when {
            purpose == ADD -> nav.popBackStack("add", false)
            viaChooser(purpose, task) -> nav.popBackStack(loadRoute(purpose, task), false)
            else -> nav.popBackStack("wallet/$purpose", false)
        }
    }
    // Words from a scan, taken as the route meant them.
    val accept = { purpose: String, task: String, words: List<String>, source: KeySource ->
        sheet.forget()
        if (purpose == ADD) {
            val id = wallets.take(words, source)
            if (id != null) open(id) else leave(purpose, task)
        } else {
            val loaded = wallets.loadInto(purpose, words, source)
            val chooser = viaChooser(purpose, task)
            val returned = leave(purpose, task)
            if (loaded && returned) {
                // Pop the method chooser to its caller; a pending review stays pending.
                if (chooser) nav.popBackStack()
                if (task == "backup" && wallets.makeShares(purpose)) nav.navigate("backup/shares")
            }
        }
    }
    val backToBackedUp = {
        val id = wallets.backup?.walletId
        wallets.endBackup()
        if (id != null) {
            nav.popBackStack("wallet/$id", false)
        } else {
            nav.popBackStack("wallets", false)
        }
    }
    NavHost(
        nav,
        startDestination = "wallets",
        enterTransition = Motion.enter,
        exitTransition = Motion.exit,
        popEnterTransition = Motion.popEnter,
        popExitTransition = Motion.popExit,
    ) {
        composable("wallets") {
            WalletsScreen(
                wallets,
                onAdd = { nav.navigate("add") },
                onOpen = { id -> nav.navigate("wallet/$id") },
            )
        }
        composable("add") {
            AddWalletScreen(
                onNewKey = {
                    if (creation.start()) nav.navigate("new/backup")
                },
                onScanPhrase = {
                    sheet.start()
                    nav.navigate(scanRoute("sheet/scan", ADD))
                },
                onScanStrips = {
                    strips.start()
                    nav.navigate(scanRoute("strips/scan", ADD))
                },
                onBack = { nav.popBackStack() },
            )
        }
        composable("wallet/{id}") { entry ->
            val id = entry.arguments?.getString("id") ?: return@composable
            WalletScreen(
                wallets,
                id,
                onBack = { nav.popBackStack("wallets", false) },
                onKeys = { nav.navigate("wallet/$id/keys") },
                onScan = {
                    sheet.start()
                    nav.navigate(scanRoute("sheet/scan", id))
                },
                onStrips = {
                    strips.start()
                    nav.navigate(scanRoute("strips/scan", id))
                },
                onConnect = { nav.navigate("wallet/$id/connect") },
                onBackUp = {
                    if (wallets.key(id) == null) {
                        nav.navigate(loadRoute(id, "backup"))
                    } else if (wallets.makeShares(id)) {
                        nav.navigate("backup/shares")
                    }
                },
                onSign = { nav.navigate("sign/pick") },
                onReview = { nav.navigate("sign/review") },
            )
        }
        composable("new/backup") {
            NewSeedScreen(creation,
                onCancel = { creation.cancel(); nav.popBackStack("add", false) },
                onComplete = { wallets.completeNew(creation)?.let(open) },
            )
        }
        composable("wallet/{id}/connect") { entry ->
            val id = entry.arguments?.getString("id") ?: return@composable
            ConnectWalletScreen(wallets, id, onBack = { nav.popBackStack() })
        }
        composable("wallet/{id}/keys") { entry ->
            val id = entry.arguments?.getString("id") ?: return@composable
            KeyControlsScreen(
                wallets, id, onBack = { nav.popBackStack() },
                onLoadKey = { nav.navigate(loadRoute(id, "keys")) },
            )
        }
        composable("load/{id}/{task}") { entry ->
            val id = entry.arguments?.getString("id") ?: return@composable
            val task = entry.task()
            LoadKeyScreen(
                wallets, id, task,
                onBack = { nav.popBackStack() },
                onScanPhrase = {
                    sheet.start()
                    nav.navigate(scanRoute("sheet/scan", id, task))
                },
                onScanStrips = {
                    strips.start()
                    nav.navigate(scanRoute("strips/scan", id, task))
                },
            )
        }
        composable("sign/pick") {
            SignPickScreen(
                wallets,
                onScanQr = { nav.navigate("sign/scan") },
                onReviewed = { reviewIncoming() },
                onBack = { nav.popBackStack() },
            )
        }
        composable("sign/scan") {
            QrScanScreen(
                "Scan the transaction",
                onPsbt = { bytes ->
                    wallets.receive(bytes)
                    reviewIncoming()
                },
                onExit = { nav.popBackStack() },
            )
        }
        composable("sign/review") {
            ReviewPsbtScreen(
                wallets,
                onSigned = { nav.navigate("sign/signed") { popUpTo("sign/review") { inclusive = true } } },
                onLoadKey = { id -> nav.navigate(loadRoute(id, "sign")) },
                onChooseAnother = {
                    wallets.dismissSigning()
                    nav.navigate("sign/pick") { popUpTo("sign/review") { inclusive = true } }
                },
                onBack = { nav.popBackStack() },
                onDone = { leaveSigning() },
            )
        }
        composable("sign/signed") {
            SignedScreen(wallets, onDone = { leaveSigning() })
        }
        composable("sheet/scan/{for}/{task}") { entry ->
            val purpose = entry.purpose()
            val task = entry.task()
            SheetScanScreen(
                sheet,
                onReviewed = { nav.navigate(scanRoute("sheet/review", purpose, task)) },
                onExit = {
                    sheet.forget()
                    leave(purpose, task)
                },
            )
        }
        composable("sheet/review/{for}/{task}") { entry ->
            val purpose = entry.purpose()
            val task = entry.task()
            ReviewScreen(
                sheet,
                action = if (purpose == ADD) "Add wallet" else "Load key",
                onAccept = {
                    sheet.confirmed()?.let { accept(purpose, task, it, KeySource.WrittenPhrase) }
                },
                onBack = {
                    sheet.rescan()
                    nav.popBackStack(scanRoute("sheet/scan", purpose, task), false)
                },
            )
        }
        composable("strips/scan/{for}/{task}") { entry ->
            val purpose = entry.purpose()
            val task = entry.task()
            StripScanScreen(
                strips,
                onRecovered = {
                    val recovered = strips.scan as? StripScan.Recovered
                    strips.finish()
                    when {
                        recovered == null -> {
                            if (purpose == CHECK) nav.popBackStack("backup/check", false)
                            else leave(purpose, task)
                        }
                        purpose == CHECK -> {
                            wallets.checkStrips(recovered.words)
                            nav.navigate("backup/checked") { popUpTo("backup/check") }
                        }
                        else -> accept(
                            purpose, task, recovered.words,
                            KeySource.Strips(recovered.first, recovered.second),
                        )
                    }
                },
                onExit = {
                    strips.cancel()
                    if (purpose == CHECK) nav.popBackStack("backup/check", false) else leave(purpose, task)
                },
            )
        }
        composable("backup/shares") {
            SharesScreen(
                wallets,
                onCheck = { nav.navigate("backup/check") },
                onDone = { backToBackedUp() },
            )
        }
        composable("backup/check") {
            if (wallets.backup == null) {
                EndedBackupScreen(onDone = { backToBackedUp() })
                return@composable
            }
            BackupCheckScreen(
                onScan = {
                    strips.start()
                    nav.navigate(scanRoute("strips/scan", CHECK))
                },
                onBack = { nav.popBackStack("backup/shares", false) },
            )
        }
        composable("backup/checked") {
            CheckedScreen(
                wallets,
                onAgain = { nav.popBackStack("backup/shares", false) },
                onRetry = {
                    strips.start()
                    nav.navigate(scanRoute("strips/scan", CHECK)) { popUpTo("backup/check") }
                },
                onDone = { backToBackedUp() },
            )
        }
    }
}
