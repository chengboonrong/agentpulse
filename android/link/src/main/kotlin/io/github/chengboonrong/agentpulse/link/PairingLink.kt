package io.github.chengboonrong.agentpulse.link

import java.net.URI
import java.net.URISyntaxException
import java.net.URLDecoder
import java.text.BreakIterator

/**
 * The QR code's link, link/'s `PairingLink`:
 * `agentpulse://pair?v=1&r=…&id=…&k=…&s=…&n=…` (docs/mobile.md, "Pairing").
 * [macName] is sanitised (see [sanitizedName]).
 */
class PairingLink private constructor(
    val relay: String,
    val id: ByteArray,
    val macKey: ByteArray,
    val secret: ByteArray,
    val macName: String,
) {
    companion object {
        /** Parses a scanned or pasted link, validating every field; the message names the bad one. */
        fun parse(string: String): PairingLink {
            val uri = try {
                URI(string)
            } catch (_: URISyntaxException) {
                throw invalid("url")
            }
            val query = uri.rawQuery
            if (uri.scheme?.lowercase() != "agentpulse" || uri.host != "pair" || !uri.rawPath.isNullOrEmpty() ||
                query == null
            ) {
                throw invalid("url")
            }
            // Split before decoding, so an encoded `&` or `=` stays in its value.
            val items = query.split("&").map {
                val i = it.indexOf('=')
                if (i < 0) decode(it) to null else decode(it.substring(0, i)) to decode(it.substring(i + 1))
            }
            fun one(name: String): String =
                items.filter { it.first == name }.singleOrNull()?.second ?: throw invalid(name)
            fun bytes(name: String, count: Int): ByteArray =
                Base64Url.decode(one(name))?.takeIf { it.size == count } ?: throw invalid(name)

            if (one("v") != "1") throw invalid("v")
            val relay = one("r")
            if (!validRelay(relay)) throw invalid("r")
            return PairingLink(
                relay, bytes("id", 16), bytes("k", 32), bytes("s", 32), sanitizedName(one("n")) ?: throw invalid("n"))
        }

        /** `https`, or `http` to `localhost` for testing; no user, query or fragment. */
        private fun validRelay(relay: String): Boolean {
            val u = try {
                URI(relay)
            } catch (_: URISyntaxException) {
                return false
            }
            val host = u.host
            return !host.isNullOrEmpty() && (u.scheme == "https" || (u.scheme == "http" && host == "localhost")) &&
                u.rawUserInfo == null && u.rawQuery == null && u.rawFragment == null
        }

        /** Percent-decoding only: unlike a form, `+` stays `+`. `URI` already refused bad escapes. */
        private fun decode(s: String): String = URLDecoder.decode(s.replace("+", "%2B"), "UTF-8")

        private fun invalid(field: String) = LinkException("invalid link: $field")
    }
}

/**
 * A device name fit to show, link/'s `sanitizedName`: Unicode control, format
 * (bidirectional marks, zero-width characters), line and paragraph separators removed,
 * trimmed, cut at a character boundary to at most 64 code points; null if nothing is left.
 */
fun sanitizedName(name: String): String? {
    val removed = setOf(
        Character.CONTROL, Character.FORMAT, Character.LINE_SEPARATOR, Character.PARAGRAPH_SEPARATOR,
    ).map { it.toInt() }
    val kept = buildString {
        name.codePoints().filter { Character.getType(it) !in removed }.forEach { appendCodePoint(it) }
    }.trim()
    val chars = BreakIterator.getCharacterInstance().apply { setText(kept) }
    var end = 0
    var next = chars.next()
    while (next != BreakIterator.DONE && kept.codePointCount(0, next) <= 64) {
        end = next
        next = chars.next()
    }
    return kept.substring(0, end).trim().ifEmpty { null }
}
