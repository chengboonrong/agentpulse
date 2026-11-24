package io.github.chengboonrong.agentpulse

import java.io.IOException

fun stateFetchFailedInTransport(error: Exception): Boolean = error is IOException

/** Two five-minute publishing cycles, including slack for an unchanged Mac. */
const val SNAPSHOT_FRESH_AGE_LIMIT_MS = 10 * 60 * 1000L

enum class SnapshotFreshness { Fresh, MacQuiet, Unreachable }

/** A successful relay response alone does not mean the Mac is awake. */
fun snapshotFreshness(generatedAtMs: Long, nowMs: Long, failed: Boolean, directOpen: Boolean = false, removed: Boolean = false): SnapshotFreshness = when {
    directOpen -> SnapshotFreshness.Fresh
    // A removed pairing shows its removal error, not "can't reach"; its counts still age.
    failed && !removed -> SnapshotFreshness.Unreachable
    nowMs - generatedAtMs <= SNAPSHOT_FRESH_AGE_LIMIT_MS -> SnapshotFreshness.Fresh
    else -> SnapshotFreshness.MacQuiet
}
