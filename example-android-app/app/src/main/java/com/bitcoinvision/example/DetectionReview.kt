package com.bitcoinvision.example

import uniffi.bitcoin_vision_mobile.DetectionSource

/** Geometry-only sources stay separate from read words and their alternatives. */
internal val DetectionSource.unread: Boolean
    get() = recoveredWord == null && parts.none { it.wordIndex != null } && union?.wordIndex == null

/** Presentation of a native decision, never another app-side keep policy. */
internal val DetectionSource.reviewDescription: String
    get() = when (decision) {
        "owned_descender", "duplicate" -> "Small ink belonging to a nearby word"
        "detached_ending" -> "Detached word ending"
        "ambiguous_owner", "other_observation", "owner_unavailable", "no_owner" -> "Small ink with no confirmed owner"
        else -> "Region that could not be read"
    }
