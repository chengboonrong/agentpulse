package io.github.chengboonrong.agentpulse

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/**
 * The widget.json contract (version 1, crates/agentpulse-app/src/widget_export.rs), as a
 * Mac's `state` carries it: the parts the app shows (macos/Shared/Payload.swift).
 */
@Serializable
data class WidgetPayload(
    val version: Int,
    @SerialName("generated_at_ms") val generatedAtMs: Long,
    val stopped: Boolean,
    val agents: Agents,
    val today: Today,
    @SerialName("output_rate") val outputRate: String,
    val cat: Cat,
    val sessions: List<Session>,
) {
    @Serializable
    data class Agents(val active: Int, val waiting: Int, val idle: Int) {
        val headline
            get() = when (val n = active + waiting + idle) {
                0 -> "No agents"
                1 -> "1 agent"
                else -> "$n agents"
            }
    }

    @Serializable
    data class Today(
        val tokens: String,
        val cost: String,
        @SerialName("cost_note") val costNote: String,
        val unpriced: Int,
    )

    @Serializable
    data class Cat(
        val state: String,
        val width: Int,
        val height: Int,
        val rows: List<String>,
        /** Sprite char to RGBA 0..255. */
        val palette: Map<String, List<Int>>,
    )

    @Serializable
    data class Session(
        val project: String,
        val status: String,
        val model: String,
        @SerialName("started_at_ms") val startedAtMs: Long? = null,
        val tokens: String,
        val cost: String,
    ) {
        /** Payloads carry no session id; a session keeps its project and start between updates. */
        val key get() = "$project ${startedAtMs ?: 0}"
    }

    companion object {
        private val json = Json { ignoreUnknownKeys = true }

        /** Version 1 only; null for anything else or anything undecodable. */
        fun decode(text: String?): WidgetPayload? = text?.let {
            runCatching { json.decodeFromString(serializer(), it) }.getOrNull()?.takeIf { p -> p.version == 1 }
        }
    }
}
