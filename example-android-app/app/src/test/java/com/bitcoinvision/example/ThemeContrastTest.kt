package com.bitcoinvision.example

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.graphics.luminance
import org.junit.Assert.assertTrue
import org.junit.Test

class ThemeContrastTest {
    private fun contrast(foreground: Color, background: Color): Float {
        val a = foreground.compositeOver(background).luminance()
        val b = background.luminance()
        return (maxOf(a, b) + 0.05f) / (minOf(a, b) + 0.05f)
    }

    @Test
    fun text_and_control_roles_meet_their_contrast_targets_in_both_themes() {
        for ((name, colors) in listOf("light" to Light, "dark" to Dark)) {
            val surfaces = listOf(colors.background, colors.surface, colors.surfaceVariant,
                colors.surfaceContainerLow, colors.surfaceContainer, colors.surfaceContainerHigh,
                colors.surfaceContainerHighest)
            for (surface in surfaces) {
                for (text in listOf(colors.onSurface, colors.onSurfaceVariant, colors.primary, colors.error)) {
                    val ratio = contrast(text, surface)
                    assertTrue("$name text contrast $ratio", ratio >= 4.5f)
                }
                val outline = contrast(colors.outline, surface)
                assertTrue("$name outline contrast $outline", outline >= 3f)
            }
            for ((text, background) in listOf(
                colors.onPrimary to colors.primary,
                colors.onPrimaryContainer to colors.primaryContainer,
                colors.onSecondaryContainer to colors.secondaryContainer,
                colors.inverseOnSurface to colors.inverseSurface,
            )) {
                val ratio = contrast(text, background)
                assertTrue("$name filled-role contrast $ratio", ratio >= 4.5f)
            }
        }
    }

    @Test
    fun camera_instruction_scrim_meets_text_contrast_on_the_brightest_preview() {
        assertTrue(contrast(Color.White, CameraScrim.compositeOver(Color.White)) >= 4.5f)
    }
}
