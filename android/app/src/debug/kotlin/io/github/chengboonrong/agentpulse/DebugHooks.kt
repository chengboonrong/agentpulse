package io.github.chengboonrong.agentpulse

import android.content.Context
import android.content.Intent
import io.github.chengboonrong.agentpulse.link.Base64Url
import io.github.chengboonrong.agentpulse.link.Pairing
import io.github.chengboonrong.agentpulse.link.PairingLink
import io.github.chengboonrong.agentpulse.link.PhoneClient
import io.github.chengboonrong.agentpulse.link.PhoneStore
import io.github.chengboonrong.agentpulse.link.SecretStore
import java.io.File
import java.net.InetSocketAddress

/**
 * Debug builds only (the release source set has a no-op). Intent extras, as `am start -e`:
 * - `fixture` `macs|mac|session|pair-confirm|pair-code|empty`: a screen from fixture data
 *   without network, for screenshots and android/e2e (ios/App/Fixtures.swift's set).
 * - `e2eLink <link>` pairs with that link, `e2eAutoConfirm true` without the Pair tap,
 *   `e2eRemove true` removes every Mac on start, and `e2eMac <ip>:<port>` is a Mac found on
 *   the network (an emulator's NSD can't see the host's): scripts/e2e-android-app.sh, with
 *   no taps. Also on a new intent, so only the extras given change anything.
 */
object DebugHooks {
    fun fixture(context: Context, intent: Intent): AppModel? {
        val name = intent.getStringExtra("fixture") ?: return null
        val store = PhoneStore(File(context.cacheDir, "fixtures"), Memory())
        val model = AppModel(PhoneClient(store), offline = true, context = context)
        val studio = pairing("Studio", minutesAgo = 1, payload = ::busy)
        // As on iPhone: Studio is connected directly, the others through the relay.
        if (name in setOf("macs", "mac", "session")) model.direct = setOf(studio.id)
        when (name) {
            "macs" -> model.pairings = listOf(
                studio, pairing("MacBook Pro", minutesAgo = 42, payload = ::quiet), pairing("Mac mini", minutesAgo = null),
            )
            "mac" -> {
                model.pairings = listOf(studio)
                model.show = studio.id
            }
            "session" -> {
                model.pairings = listOf(studio)
                model.show = Nav.session(studio.id, WidgetPayload.decode(studio.lastState)!!.sessions.first().key)
            }
            "pair-confirm" -> model.pairRequest = PairRequest(step = PairStep.Confirm(PairingLink.parse(LINK), LINK))
            "pair-code" -> model.pairRequest = PairRequest(step = PairStep.Code("042137", pairing("Studio", null)))
        }
        return model
    }

    fun e2e(model: AppModel, intent: Intent) {
        intent.getStringExtra("e2eAutoConfirm")?.let { model.autoConfirm = it == "true" }
        intent.getStringExtra("e2eRemove")?.let { model.removeOnStart = it == "true" }
        intent.getStringExtra("e2eLink")?.let { model.open(it) }
        // A numeric address, which needs no lookup on the main thread.
        intent.getStringExtra("e2eMac")?.let {
            val port = it.substringAfterLast(':').toIntOrNull() ?: return
            model.found(listOf(InetSocketAddress(it.substringBeforeLast(':'), port)))
        }
    }

    private val LINK = "agentpulse://pair?v=1&r=https://relay.example.com" +
        "&id=${Base64Url.encode(ByteArray(16) { 1 })}&k=${Base64Url.encode(ByteArray(32) { 9 })}" +
        "&s=${Base64Url.encode(ByteArray(32) { 2 })}&n=Studio"

    private fun pairing(name: String, minutesAgo: Int?, payload: (Long) -> String = { "" }): Pairing {
        val at = minutesAgo?.let { System.currentTimeMillis() - it * 60_000L }
        return Pairing(
            id = name, relay = "https://relay.example.com", macName = name,
            macKey = Base64Url.encode(ByteArray(32) { 3 }),
            lastState = at?.let(payload), lastStateAtMs = at,
        )
    }

    private fun busy(ms: Long) = payload(
        ms, agents = Triple(2, 1, 1),
        sessions = """
            {"project":"agentpulse","status":"Waiting","model":"opus","started_at_ms":${ms - 1_520_000},"tokens":"1.2M","cost":"$4.10"},
            {"project":"kl-rush","status":"Active","model":"gpt-5","started_at_ms":${ms - 610_000},"tokens":"380k","cost":"$1.25"},
            {"project":"relay","status":"Active","model":"sonnet","started_at_ms":${ms - 95_000},"tokens":"41k","cost":"$0.12"},
            {"project":"notes","status":"Idle","model":"haiku","started_at_ms":${ms - 3_400_000},"tokens":"9k","cost":"$0.01"}
        """,
    )

    private fun quiet(ms: Long) = payload(ms, agents = Triple(0, 0, 0), sessions = "")

    private fun payload(ms: Long, agents: Triple<Int, Int, Int>, sessions: String) = """
        {"version":1,"generated_at_ms":$ms,"stopped":false,
        "agents":{"active":${agents.first},"waiting":${agents.second},"idle":${agents.third}},
        "today":{"tokens":"2.1M","cost":"$6.40","cost_note":"Estimated from list prices.","unpriced":0},
        "output_rate":"38 tok/s",
        "cat":{"state":"sleeping","width":32,"height":16,"rows":[
        "................................","................................","................................",
        "................................","................................","......................o....o....",
        ".............o..oooooobo..obo...","............osoosbbsbbbboobbo...","...........oobsbbsbbsbbbbbbbbo..",
        "..........obbbbbbbbbbbbbbeebbo..","..........obbbbbbbbbbbbbbbwwpo..","..........obbbbbbbbbbbbbbbboo...",
        "..........obbbwwwbbbbwwwbbbo....","...........osssbbwwwwbsssoo.....","............ooosssssssooo.......",
        "...............ooooooo.........."],
        "palette":{"b":[232,140,56,255],"e":[30,24,20,255],"o":[150,78,30,255],"p":[240,120,140,255],"s":[176,88,28,255],"w":[250,222,176,255]}},
        "sessions":[$sessions]}
    """

    private class Memory : SecretStore {
        private val items = mutableMapOf<String, ByteArray>()
        override fun read(account: String) = items[account]
        override fun write(account: String, data: ByteArray) {
            items[account] = data
        }
        override fun delete(account: String) {
            items.remove(account)
        }
    }
}
