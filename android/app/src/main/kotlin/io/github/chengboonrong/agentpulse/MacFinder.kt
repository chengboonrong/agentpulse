package io.github.chengboonrong.agentpulse

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Handler
import android.os.Looper
import java.net.InetSocketAddress

/**
 * Finds AgentPulse Macs on the local network: NSD (DNS-SD) `_agentpulse._tcp` with TXT
 * `v=1`, link/'s `LinkBrowser`. [start]'s `onChange` runs on the main thread with every
 * Mac currently resolved. The service names say nothing about pairings, so every one is
 * tried; a Mac that doesn't know the pairing closes.
 */
class MacFinder(context: Context) {
    private val nsd = context.getSystemService(NsdManager::class.java)
    private val main = Handler(Looper.getMainLooper())
    // Main thread only.
    private var listener: NsdManager.DiscoveryListener? = null
    private var onChange: (List<InetSocketAddress>) -> Unit = {}
    private val found = linkedMapOf<String, InetSocketAddress>()
    private val pending = ArrayDeque<NsdServiceInfo>()
    private var resolving = false

    fun start(onChange: (List<InetSocketAddress>) -> Unit) {
        if (listener != null) return
        this.onChange = onChange
        val l = object : NsdManager.DiscoveryListener {
            override fun onServiceFound(info: NsdServiceInfo) {
                main.post { if (listener === this) queue(info) }
            }

            override fun onServiceLost(info: NsdServiceInfo) {
                main.post { if (listener === this) lost(info.serviceName) }
            }

            override fun onDiscoveryStarted(type: String) {}
            override fun onDiscoveryStopped(type: String) {}
            override fun onStartDiscoveryFailed(type: String, error: Int) {}
            override fun onStopDiscoveryFailed(type: String, error: Int) {}
        }
        listener = l
        nsd.discoverServices(TYPE, NsdManager.PROTOCOL_DNS_SD, l)
    }

    fun stop() {
        val l = listener ?: return
        listener = null
        runCatching { nsd.stopServiceDiscovery(l) }
        found.clear()
        pending.clear()
    }

    private fun queue(info: NsdServiceInfo) {
        pending.addLast(info)
        resolveNext()
    }

    private fun lost(name: String) {
        pending.removeAll { it.serviceName == name }
        if (found.remove(name) != null) onChange(found.values.toList())
    }

    // ponytail: the pre-34 resolveService, which takes one at a time, on every version;
    // registerServiceInfoCallback (34+) would also follow a Mac whose address changes.
    @Suppress("DEPRECATION")
    private fun resolveNext() {
        if (resolving) return
        val next = pending.removeFirstOrNull() ?: return
        resolving = true
        val session = listener
        nsd.resolveService(next, object : NsdManager.ResolveListener {
            override fun onServiceResolved(info: NsdServiceInfo) {
                main.post {
                    resolving = false
                    val host = info.host
                    // Unless stopped, or stopped and started again, meanwhile.
                    if (listener === session && host != null && info.attributes["v"]?.decodeToString() == "1") {
                        found[info.serviceName] = InetSocketAddress(host, info.port)
                        onChange(found.values.toList())
                    }
                    resolveNext()
                }
            }

            override fun onResolveFailed(info: NsdServiceInfo, error: Int) {
                main.post {
                    resolving = false
                    resolveNext()
                }
            }
        })
    }

    private companion object {
        const val TYPE = "_agentpulse._tcp"
    }
}
