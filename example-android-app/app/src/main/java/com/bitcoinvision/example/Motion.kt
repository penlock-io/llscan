package com.bitcoinvision.example

import androidx.compose.animation.AnimatedContentTransitionScope
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.navigation.NavBackStackEntry

/**
 * The app's one motion vocabulary. State changes settle on a spring; page
 * changes slide an eighth of the width and fade, reversed on the way back.
 * Compose scales every duration by the system animator setting, so a
 * reduced-motion device gets reduced motion without a second code path.
 */
object Motion {
    /** For content that changes in place: no overshoot, so nothing wobbles. */
    fun <T> state() = spring<T>(
        dampingRatio = Spring.DampingRatioNoBouncy,
        stiffness = Spring.StiffnessMediumLow,
    )

    private const val PAGE_MS = 220
    private const val PAGE_OUT_MS = 160
    private fun pageIn() = tween<Float>(PAGE_MS, easing = FastOutSlowInEasing)
    private fun pageOut() = tween<Float>(PAGE_OUT_MS, easing = FastOutSlowInEasing)
    private fun slide() = tween<androidx.compose.ui.unit.IntOffset>(PAGE_MS, easing = FastOutSlowInEasing)

    val enter: AnimatedContentTransitionScope<NavBackStackEntry>.() -> EnterTransition = {
        slideInHorizontally(slide()) { it / 8 } + fadeIn(pageIn())
    }
    val exit: AnimatedContentTransitionScope<NavBackStackEntry>.() -> ExitTransition = {
        slideOutHorizontally(slide()) { -it / 8 } + fadeOut(pageOut())
    }
    val popEnter: AnimatedContentTransitionScope<NavBackStackEntry>.() -> EnterTransition = {
        slideInHorizontally(slide()) { -it / 8 } + fadeIn(pageIn())
    }
    val popExit: AnimatedContentTransitionScope<NavBackStackEntry>.() -> ExitTransition = {
        slideOutHorizontally(slide()) { it / 8 } + fadeOut(pageOut())
    }
}
