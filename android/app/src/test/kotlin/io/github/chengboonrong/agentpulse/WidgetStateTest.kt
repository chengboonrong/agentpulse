package io.github.chengboonrong.agentpulse

import io.github.chengboonrong.agentpulse.link.Pairing
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs

class WidgetStateTest {
    private val now = 1_800_000_000_000L
    private val mac = Pairing("id", "https://relay.example", "Studio", "key")

    private fun state(generatedAtMs: Long, stopped: Boolean = false, version: Int = 1) = """
        {"version":$version,"generated_at_ms":$generatedAtMs,"stopped":$stopped,
         "agents":{"active":1,"waiting":0,"idle":0},
         "today":{"tokens":"1.2M","cost":"$3.40","cost_note":"","unpriced":0},
         "output_rate":"0","cat":{"state":"idle","width":1,"height":1,"rows":["a"],"palette":{}},
         "sessions":[],"extra":true}
    """

    @Test
    fun followsTheFirstMacsStoredState() {
        assertEquals(WidgetState.Unpaired, WidgetState.of(null, now))
        assertEquals(WidgetState.Waiting("Studio"), WidgetState.of(mac, now))
        assertEquals(WidgetState.NotRunning, WidgetState.of(mac.copy(lastState = "not json"), now))
        assertEquals(WidgetState.NotRunning, WidgetState.of(mac.copy(lastState = state(now, version = 2)), now))
        assertEquals(WidgetState.NotRunning, WidgetState.of(mac.copy(lastState = state(now, stopped = true)), now))
        val live = WidgetState.of(mac.copy(lastState = state(now - WidgetState.AGE_LIMIT_MS)), now)
        assertIs<WidgetState.Live>(live)
        assertEquals("$3.40", live.payload.today.cost)
        val old = now - WidgetState.AGE_LIMIT_MS - 1
        assertEquals(WidgetState.Stale(old), WidgetState.of(mac.copy(lastState = state(old)), now))
    }
}
