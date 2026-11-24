package io.github.chengboonrong.agentpulse

import java.io.IOException
import io.github.chengboonrong.agentpulse.link.PhoneException
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlin.test.Test
import kotlin.test.assertEquals

class SnapshotFreshnessTest {
    private val now = 1_800_000_000_000L

    @Test fun freshAndBoundary() {
        assertEquals(SnapshotFreshness.Fresh, snapshotFreshness(now - 60_000, now, false))
        assertEquals(SnapshotFreshness.Fresh, snapshotFreshness(now - 600_000, now, false))
    }

    @Test fun quietAfterTenMinutes() {
        assertEquals(SnapshotFreshness.MacQuiet, snapshotFreshness(now - 600_001, now, false))
    }

    @Test fun failureOverridesAge() {
        assertEquals(SnapshotFreshness.Unreachable, snapshotFreshness(now, now, true))
        assertEquals(SnapshotFreshness.Unreachable, snapshotFreshness(now - 3_600_000, now, true))
    }

    @Test fun recovery() {
        assertEquals(SnapshotFreshness.Unreachable, snapshotFreshness(now - 3_600_000, now, true))
        assertEquals(SnapshotFreshness.MacQuiet, snapshotFreshness(now - 3_600_000, now, false))
        assertEquals(SnapshotFreshness.Fresh, snapshotFreshness(now, now, false))
    }
    @Test fun connectionAndRemovalOverrideTransportFailure() {
        val old = now - 3_600_000
        assertEquals(SnapshotFreshness.Fresh, snapshotFreshness(old, now, true, directOpen = true))
        assertEquals(SnapshotFreshness.MacQuiet, snapshotFreshness(old, now, true, removed = true))
        // A relay 5xx is an HTTP response, not a transport failure.
        assertEquals(SnapshotFreshness.MacQuiet, snapshotFreshness(old, now, false))
    }
    @Test fun onlyTransportErrorsFailTheStateFetch() {
        assertTrue(stateFetchFailedInTransport(IOException("offline")))
        assertFalse(stateFetchFailedInTransport(PhoneException(PhoneException.Reason.BAD_RESPONSE, 500)))
        assertFalse(stateFetchFailedInTransport(PhoneException(PhoneException.Reason.BUSY, 429)))
        assertFalse(stateFetchFailedInTransport(PhoneException(PhoneException.Reason.REMOVED)))
    }
}
