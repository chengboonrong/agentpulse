package io.github.chengboonrong.agentpulse

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.chengboonrong.agentpulse.link.DirectConnection
import io.github.chengboonrong.agentpulse.link.LinkException
import io.github.chengboonrong.agentpulse.link.Pairing
import io.github.chengboonrong.agentpulse.link.PairingLink
import io.github.chengboonrong.agentpulse.link.PhoneClient
import java.io.IOException
import java.net.InetSocketAddress
import java.net.URI
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Where the Pair screen is: scan or paste, confirm the Mac, or the code while waiting. */
sealed interface PairStep {
    data object Scan : PairStep
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
 * Mac on start, then a direct connection to each Mac found on the network ([finder]),
 * else `GET down` every 10 s through the relay.
 */
class AppModel(val client: PhoneClient, val offline: Boolean = false, private val finder: MacFinder? = null) : ViewModel() {
    var pairings by mutableStateOf(emptyList<Pairing>())
    /** Pairing ids connected directly. */
    var direct by mutableStateOf(emptySet<String>())
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
    // Main thread only.
    private var macs = emptyList<InetSocketAddress>()
    private val connections = mutableMapOf<String, DirectConnection>()
    private val connecting = mutableSetOf<String>()

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
                // At once, so one unreachable Mac or relay doesn't hold up the others.
                coroutineScope {
                    pairings.filter { it.id !in direct }.forEach {
                        launch {
                            connectDirect(it.id)
                            if (it.id !in direct) fetch(it.id)
                        }
                    }
                }
                delay(10_000)
            }
        }
        browse()
    }

    fun stop() {
        refresh?.cancel()
        refresh = null
        finder?.stop()
        macs = emptyList()
        connections.values.forEach { it.close() }
        connections.clear()
        direct = emptySet()
    }

    override fun onCleared() = stop()

    /** A pairing that just got its first state. */
    fun added(p: Pairing) {
        pairRequest = null
        reload()
        show = p.id
        browse()
        viewModelScope.launch {
            seen(p)
            connectDirect(p.id)
        }
    }

    /**
     * Out of the model first, so nothing sends a newer `seen` after the `unpair`; in the
     * model's scope, as the screen that asked goes with it.
     */
    fun remove(p: Pairing): Job {
        pairings = pairings.filter { it.id != p.id }
        problems = problems - p.id
        connections.remove(p.id)?.close()
        direct = direct - p.id
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

    /** NSD only once a Mac is paired, and while in the foreground. */
    private fun browse() {
        if (refresh == null || pairings.isEmpty()) return
        finder?.start(::found)
    }

    /** The Macs on the network now, from NSD (or a debug build's e2e hook). */
    fun found(addresses: List<InetSocketAddress>) {
        macs = addresses
        pairings.filter { it.id !in direct }.forEach { viewModelScope.launch { connectDirect(it.id) } }
    }

    /** Tries each Mac found; one that doesn't know the pairing closes. */
    private suspend fun connectDirect(id: String) {
        if (id in connecting || id in connections) return
        connecting += id
        try {
            for (address in macs) {
                val p = pairing(id) ?: return
                val c = try {
                    // Not cancellable: a connection made after stop() must still be closed below.
                    withContext(NonCancellable + Dispatchers.IO) { client.connect(p, address) }
                } catch (e: CancellationException) {
                    throw e
                } catch (_: Exception) {
                    continue
                }
                // Stopped or removed meanwhile.
                if (refresh == null || pairing(id) == null) return c.close()
                connections[id] = c
                direct = direct + id
                problems = problems - id
                viewModelScope.launch { listen(id, c) }
                return
            }
        } finally {
            connecting -= id
        }
    }

    private suspend fun listen(id: String, c: DirectConnection) {
        try {
            while (true) {
                val p = io { c.next() }
                if (pairing(id) != null) pairings = pairings.map { if (it.id == id) p else it }
            }
        } catch (e: CancellationException) {
            c.close()
            throw e
        } catch (_: Exception) {
            // Closed: back to the relay on the next tick.
        }
        // Unless stop() or a newer connection already replaced it.
        if (connections[id] !== c) return
        connections.remove(id)
        direct = direct - id
    }

    private suspend fun seen(p: Pairing) = report(p.id) { io { client.post(PhoneClient.Up.SEEN, p) } }

    private suspend fun fetch(id: String) {
        val p = pairing(id) ?: return
        report(id) {
            val fetched = io { client.fetchDown(p) }
            // Unless it was removed, or connected directly (newer), meanwhile.
            if (pairing(id) != null && id !in direct) pairings = pairings.map { if (it.id == id) fetched else it }
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
