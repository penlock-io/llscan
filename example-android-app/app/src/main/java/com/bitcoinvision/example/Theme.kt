package com.bitcoinvision.example

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight

/** The worksheet's face: what is written on paper and read off it. */
val PlexMono = FontFamily(Font(R.font.ibm_plex_mono))

/** The prose face, the worksheet face's sans companion. */
@OptIn(ExperimentalTextApi::class)
val PlexSans = FontFamily(
    Font(
        R.font.ibm_plex_sans, FontWeight.Normal,
        variationSettings = FontVariation.Settings(FontVariation.weight(400)),
    ),
    Font(
        R.font.ibm_plex_sans, FontWeight.Medium,
        variationSettings = FontVariation.Settings(FontVariation.weight(500)),
    ),
)

// Paper, ink, graphite and one pen: the app is the digital half of a
// paper worksheet, so it keeps the worksheet's colours rather than
// the phone's wallpaper's.
private val Paper = Color(0xFFFFFFFF)
private val PaperShade = Color(0xFFF1F1EF)
private val Ink = Color(0xFF17181A)
private val Graphite = Color(0xFF5C5F63)
private val Rule = Color(0xFFD9DAD6)
private val Pen = Color(0xFF2447A0)
private val PenTint = Color(0xFFDCE4F7)
private val PenInk = Color(0xFF14295E)

internal val Light = lightColorScheme(
    primary = Pen,
    onPrimary = Paper,
    primaryContainer = PenTint,
    onPrimaryContainer = PenInk,
    secondary = Graphite,
    onSecondary = Paper,
    secondaryContainer = PaperShade,
    onSecondaryContainer = Ink,
    tertiary = Graphite,
    onTertiary = Paper,
    tertiaryContainer = PaperShade,
    onTertiaryContainer = Ink,
    background = Color(0xFFF7F7F5),
    onBackground = Ink,
    surface = Paper,
    onSurface = Ink,
    surfaceVariant = PaperShade,
    onSurfaceVariant = Graphite,
    surfaceContainerLowest = Paper,
    surfaceContainerLow = Color(0xFFF7F7F5),
    surfaceContainer = PaperShade,
    surfaceContainerHigh = Color(0xFFEAEAE7),
    surfaceContainerHighest = Color(0xFFE3E3E0),
    outline = Color(0xFF73767A),
    outlineVariant = Rule,
)

private val Night = Color(0xFF131315)
private val NightShade = Color(0xFF1C1C1F)
private val Chalk = Color(0xFFECECEA)
private val Ash = Color(0xFFB0B2B6)
private val PenLight = Color(0xFF9DB4F2)

internal val Dark = darkColorScheme(
    primary = PenLight,
    onPrimary = Color(0xFF0E1F4D),
    primaryContainer = Color(0xFF1F3574),
    onPrimaryContainer = PenTint,
    secondary = Ash,
    onSecondary = Night,
    secondaryContainer = Color(0xFF2C2D31),
    onSecondaryContainer = Chalk,
    tertiary = Ash,
    onTertiary = Night,
    tertiaryContainer = Color(0xFF2C2D31),
    onTertiaryContainer = Chalk,
    background = Night,
    onBackground = Chalk,
    surface = Night,
    onSurface = Chalk,
    surfaceVariant = Color(0xFF26272B),
    onSurfaceVariant = Ash,
    surfaceContainerLowest = Color(0xFF0E0E10),
    surfaceContainerLow = Color(0xFF18181B),
    surfaceContainer = NightShade,
    surfaceContainerHigh = Color(0xFF232427),
    surfaceContainerHighest = Color(0xFF2C2D31),
    outline = Color(0xFF8A8D91),
    outlineVariant = Color(0xFF34363A),
)

/** White camera instructions retain contrast even over a white preview. */
internal val CameraScrim = Color.Black.copy(alpha = 0.65f)

private fun sans(style: TextStyle, weight: FontWeight = FontWeight.Normal) =
    style.copy(fontFamily = PlexSans, fontWeight = weight)

private fun mono(style: TextStyle) = style.copy(fontFamily = PlexMono)

private val Type = Typography().let { base ->
    Typography(
        displayLarge = mono(base.displayLarge),
        displayMedium = mono(base.displayMedium),
        displaySmall = mono(base.displaySmall),
        headlineLarge = sans(base.headlineLarge, FontWeight.Medium),
        headlineMedium = sans(base.headlineMedium, FontWeight.Medium),
        headlineSmall = sans(base.headlineSmall, FontWeight.Medium),
        titleLarge = sans(base.titleLarge, FontWeight.Medium),
        titleMedium = sans(base.titleMedium, FontWeight.Medium),
        titleSmall = sans(base.titleSmall, FontWeight.Medium),
        bodyLarge = sans(base.bodyLarge),
        bodyMedium = sans(base.bodyMedium),
        bodySmall = sans(base.bodySmall),
        labelLarge = sans(base.labelLarge, FontWeight.Medium),
        labelMedium = sans(base.labelMedium, FontWeight.Medium),
        labelSmall = sans(base.labelSmall, FontWeight.Medium),
    )
}

@Composable
fun VisionTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = if (isSystemInDarkTheme()) Dark else Light,
        typography = Type,
        content = content,
    )
}
