package com.bitcoinvision.example

/** The one name rule shared by input validation and persistence. */
fun normalizedWalletName(input: String): String? {
    val name = input.trim()
    return name.takeIf {
        it.isNotEmpty() && it.codePointCount(0, it.length) <= 40 &&
            it.none { character -> Character.isISOControl(character) }
    }
}
