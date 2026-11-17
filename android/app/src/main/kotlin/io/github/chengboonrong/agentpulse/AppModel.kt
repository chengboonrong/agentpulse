package io.github.chengboonrong.agentpulse

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.chengboonrong.agentpulse.link.LinkException
import io.github.chengboonrong.agentpulse.link.Pairing
import io.github.chengboonrong.agentpulse.link.PairingLink
import io.github.chengboonrong.agentpulse.link.PhoneClient
import java.io.IOException
import java.net.URI
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Where the Pair screen is: paste, confirm the Mac, or the code while waiting. */
sealed interface PairStep {
    data object Paste : PairStep
    /** [raw]: the link as scanned or pasted, which the hello is made from. */
    data class Confirm(val link: PairingLink, val raw: String) : PairStep
    data class Code(val code: String, val pairing: Pairing) : PairStep
}

/**
 * Pair on screen: from "Pair a Mac" (no link) or an opened link; fixtures start at a step.
 * In the model, so a rotation keeps the step, and the code, rather than pairing twice.
 */
data class PairRequest(val link: String? = null, val step: PairStep? = null, val error: String? = null)

/**
 * The paired Macs while the app is in the foreground (docs/mobile.md): `seen` to each
 * Mac on start, then `GET down` every 10 s through the relay. The direct connection
 * comes later.
 */
class AppModel(val client: PhoneClient, val offline: Boolean = false) : ViewModel() {
    var pairings by mutableStateOf(emptyList<Pairing>())
    /** Pairing id to the last error, such as "This Mac removed the pairing." */
    var problems by mutableStateOf(emptyMap<String, String>())
    /** The stored pairings couldn't be read. */
    var loadError by mutableStateOf<String?>(null)
    var pairRequest by mutableStateOf<PairRequest?>(null)
    /** The hello is on its way. */
    var sendingHello by mutableStateOf(false)
        private set
    /** A pane to open the panes at (a [Nav] key): a fixture, or a Mac just paired. */
    var show by mutableStateOf<String?>(null)
    /** Bumped when a Mac is removed, so the panes start over from the list. */
    var generation by mutableIntStateOf(0)

    /** Debug builds, scripts/e2e-android-app.sh: pair with no tap; remove every Mac on start. */
    var autoConfirm = false
    var removeOnStart = false

    private var refresh: Job? = null

    init {
        if (!offline) reload()
    }

    fun pairing(id: String) = pairings.firstOrNull { it.id == id }

    /** A scanned or pasted link: confirm the Mac. */
    fun take(raw: String, deviceName: String) {
        val s = raw.trim()
        val step = try {
            PairStep.Confirm(PairingLink.parse(s), s)
        } catch (_: LinkException) {
            pairRequest = pairRequest?.copy(error = "That isn't an AgentPulse pairing link.")
            return
        }
        pairRequest = pairRequest?.copy(step = step, error = null)
        if (autoConfirm) pair(s, deviceName)
    }

    /** Sends the hello, then shows the code. */
    fun pair(raw: String, deviceName: String) {
        if (sendingHello || offline) return
        val request = pairRequest ?: return
        sendingHello = true
        viewModelScope.launch {
            try {
                val (code, p) = io { client.pair(raw, deviceName) }
                reload()
                // Unless cancelled meanwhile.
                if (pairRequest == request) pairRequest = request.copy(step = PairStep.Code(code, p), error = null)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                if (pairRequest == request) pairRequest = request.copy(error = problem(e))
            } finally {
                sendingHello = false
            }
        }
    }

    /** An opened link: pair links show Pair. */
    fun open(link: String) {
        if (runCatching { URI(link).host }.getOrNull() == "pair") pairRequest = PairRequest(link = link)
    }

    fun start() {
        if (offline || refresh != null) return
        reload()
        if (removeOnStart) {
            refresh = viewModelScope.launch { pairings.forEach { remove(it).join() } }
            return
        }
        pairings.forEach { p -> viewModelScope.launch { seen(p) } }
        refresh = viewModelScope.launch {
            while (isActive) {
                // At once, so one unreachable relay doesn't hold up the other Macs.
                coroutineScope { pairings.forEach { launch { fetch(it.id) } } }
                delay(10_000)
            }
        }
    }

    fun stop() {
        refresh?.cancel()
        refresh = null
    }

    /** A pairing that just got its first state. */
    fun added(p: Pairing) {
        pairRequest = null
        reload()
        show = p.id
        viewModelScope.launch { seen(p) }
    }

    /**
     * Out of the model first, so nothing sends a newer `seen` after the `unpair`; in the
     * model's scope, as the screen that asked goes with it.
     */
    fun remove(p: Pairing): Job {
        pairings = pairings.filter { it.id != p.id }
        problems = problems - p.id
        generation++
        return viewModelScope.launch {
            io { client.remove(p) }
            reload()
        }
    }

    fun reload() {
        try {
            pairings = client.store.pairings()
            loadError = null
        } catch (e: Exception) {
            loadError = e.message ?: "The stored Macs couldn't be read."
        }
    }

    private suspend fun seen(p: Pairing) = report(p.id) { io { client.post(PhoneClient.Up.SEEN, p) } }

    private suspend fun fetch(id: String) {
        val p = pairing(id) ?: return
        report(id) {
            val fetched = io { client.fetchDown(p) }
            // Unless it was removed meanwhile.
            if (pairing(id) != null) pairings = pairings.map { if (it.id == id) fetched else it }
        }
    }

    private suspend fun report(id: String, body: suspend () -> Unit) {
        try {
            body()
            problems = problems - id
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (pairing(id) != null) problems = problems + (id to problem(e))
        }
    }
}

/** What to tell the user about an error. */
fun problem(e: Exception): String =
    if (e is IOException) "Couldn't reach the relay." else e.message ?: "Something went wrong."

suspend fun <T> io(body: () -> T): T = withContext(Dispatchers.IO) { body() }
