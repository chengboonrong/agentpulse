@file:OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3AdaptiveApi::class)

package io.github.chengboonrong.agentpulse

import android.text.format.DateFormat
import android.text.format.DateUtils
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.adaptive.ExperimentalMaterial3AdaptiveApi
import androidx.compose.material3.adaptive.currentWindowAdaptiveInfoV2
import androidx.compose.material3.adaptive.layout.AnimatedPane
import androidx.compose.material3.adaptive.layout.ListDetailPaneScaffoldRole
import androidx.compose.material3.adaptive.layout.PaneAdaptedValue
import androidx.compose.material3.adaptive.layout.ThreePaneScaffoldDestinationItem
import androidx.compose.material3.adaptive.layout.calculatePaneScaffoldDirective
import androidx.compose.material3.adaptive.navigation.NavigableListDetailPaneScaffold
import androidx.compose.material3.adaptive.navigation.rememberListDetailPaneScaffoldNavigator
import androidx.compose.ui.res.stringResource
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.chengboonrong.agentpulse.link.Pairing
import java.util.Date
import kotlin.math.floor
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * The navigator's keys: a Mac's pairing id (its pane), or the id and a session's key on
 * two lines (the session's pane).
 */
object Nav {
    fun session(mac: String, session: String) = "$mac\n$session"
    fun mac(key: String?) = key?.substringBefore('\n')
    fun session(key: String?) = key?.substringAfter('\n', "")?.ifEmpty { null }
}

/**
 * Compact width: the Macs, then a Mac, then a session, one at a time. From 600 dp (tablets,
 * unfolded foldables, phones in landscape) two panes, from 1200 dp three: the Macs, the
 * selected Mac (the first until one is chosen) and the selected session.
 */
@Composable
fun AppScreen(model: AppModel) {
    model.pairRequest?.let {
        PairScreen(model, it)
        return
    }
    if (model.loadError != null || model.pairings.isEmpty()) {
        MacsPane(model, selected = null, onPick = {})
        return
    }
    key(model.generation) { Panes(model) }
}

@Composable
private fun Panes(model: AppModel) {
    val width = LocalConfiguration.current.screenWidthDp
    val directive = calculatePaneScaffoldDirective(currentWindowAdaptiveInfoV2()).copy(
        maxHorizontalPartitions = if (width >= 1200) 3 else if (width >= 600) 2 else 1,
    )
    // `show` is set only before the panes appear (a fixture, or a Mac just paired), so it seeds
    // the history: navigating once composed left one pane showing the list.
    val show = remember { model.show }
    val navigator = rememberListDetailPaneScaffoldNavigator(
        scaffoldDirective = directive,
        initialDestinationHistory = listOfNotNull(
            ThreePaneScaffoldDestinationItem<String>(ListDetailPaneScaffoldRole.List),
            show?.let { ThreePaneScaffoldDestinationItem(ListDetailPaneScaffoldRole.Detail, Nav.mac(it)) },
            show?.takeIf { Nav.session(it) != null }?.let { ThreePaneScaffoldDestinationItem(ListDetailPaneScaffoldRole.Extra, it) },
        ),
    )
    LaunchedEffect(Unit) { model.show = null }
    val scope = rememberCoroutineScope()
    val current = navigator.currentDestination?.contentKey
    val mac = Nav.mac(current)?.takeIf { model.pairing(it) != null } ?: model.pairings.first().id
    val session = Nav.session(current)
    fun hidden(role: androidx.compose.material3.adaptive.layout.ThreePaneScaffoldRole) =
        navigator.scaffoldValue[role] == PaneAdaptedValue.Hidden
    val back: () -> Unit = { scope.launch { navigator.navigateBack() } }

    NavigableListDetailPaneScaffold(
        navigator = navigator,
        listPane = {
            AnimatedPane {
                MacsPane(
                    model, selected = if (hidden(ListDetailPaneScaffoldRole.Detail)) null else mac,
                    onPick = { scope.launch { navigator.navigateTo(ListDetailPaneScaffoldRole.Detail, it) } },
                )
            }
        },
        detailPane = {
            AnimatedPane {
                MacPane(
                    model, mac, selected = if (hidden(ListDetailPaneScaffoldRole.Extra)) null else session,
                    back = if (hidden(ListDetailPaneScaffoldRole.List)) back else null,
                    onPick = {
                        scope.launch {
                            // Through the Mac, so two panes show the Mac and the session, not the Macs.
                            if (navigator.currentDestination?.pane != ListDetailPaneScaffoldRole.Detail) {
                                navigator.navigateTo(ListDetailPaneScaffoldRole.Detail, mac)
                            }
                            navigator.navigateTo(ListDetailPaneScaffoldRole.Extra, Nav.session(mac, it))
                        }
                    },
                )
            }
        },
        extraPane = {
            AnimatedPane {
                SessionPane(model, mac, session, back = if (hidden(ListDetailPaneScaffoldRole.Detail)) back else null)
            }
        },
    )
}

// MARK: The Macs

@Composable
fun MacsPane(model: AppModel, selected: String?, onPick: (String) -> Unit) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.macs)) },
                actions = {
                    if (model.pairings.isNotEmpty()) {
                        TextButton(onClick = { model.pairRequest = PairRequest() }) { Text(stringResource(R.string.pair_mac)) }
                    }
                },
            )
        },
    ) { padding ->
        val error = model.loadError
        when {
            error != null -> Message(padding, stringResource(R.string.cant_read), error)
            model.pairings.isEmpty() -> Message(
                padding, stringResource(R.string.no_macs),
                stringResource(R.string.setup),
                textAlign = TextAlign.Start,
            ) {
                Button(onClick = { model.pairRequest = PairRequest() }) { Text(stringResource(R.string.pair_mac)) }
            }
            else -> LazyColumn(contentPadding = padding) {
                items(model.pairings, key = { it.id }) { p ->
                    MacRow(p, model.problems[p.id], p.id in model.direct, selected = p.id == selected, onClick = { onPick(p.id) })
                    HorizontalDivider()
                }
            }
        }
    }
}

@Composable
private fun MacRow(pairing: Pairing, problem: String?, direct: Boolean, selected: Boolean, onClick: () -> Unit) {
    val payload = WidgetPayload.decode(pairing.lastState)
    val now by rememberNow(30_000)
    Column(
        Modifier
            .fillMaxWidth()
            .background(if (selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent)
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Text(pairing.macName, style = MaterialTheme.typography.titleMedium)
        if (payload != null) {
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(payload.agents.headline(LocalResources.current), style = MaterialTheme.typography.bodyMedium)
                if (payload.agents.waiting > 0) Chip(stringResource(R.string.need_count, payload.agents.waiting), Palette.need)
            }
        }
        val at = pairing.lastStateAtMs
        Text(
            when {
                problem != null -> problem
                at != null && now - at < 60_000 -> stringResource(if (direct) R.string.updated_now_direct else R.string.updated_now)
                at != null -> stringResource(if (direct) R.string.updated_direct else R.string.updated, DateUtils.getRelativeTimeSpanString(at, now, DateUtils.MINUTE_IN_MILLIS))
                else -> stringResource(R.string.waiting_mac)
            },
            style = MaterialTheme.typography.bodySmall,
            color = if (problem != null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

// MARK: A Mac

@Composable
private fun MacPane(model: AppModel, id: String, selected: String?, back: (() -> Unit)?, onPick: (String) -> Unit) {
    val pairing = model.pairing(id)
    var confirming by remember { mutableStateOf(false) }
    val context = LocalContext.current
    val resources = LocalResources.current
    Scaffold(topBar = { Bar(pairing?.macName ?: stringResource(R.string.removed), back) }) { padding ->
        if (pairing == null) {
            Message(padding, stringResource(R.string.removed), "")
            return@Scaffold
        }
        val payload = WidgetPayload.decode(pairing.lastState)
        LazyColumn(contentPadding = padding) {
            model.problems[id]?.let { problem ->
                item { Text(problem, Modifier.padding(16.dp), color = MaterialTheme.colorScheme.error) }
            }
            if (payload == null) {
                item { Text(stringResource(R.string.first_update), Modifier.padding(16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant) }
            } else {
                header(payload.agents.headline(resources))
                item {
                    CatView(
                        payload.cat,
                        Modifier
                            .padding(horizontal = 16.dp)
                            .fillMaxWidth()
                            .height(120.dp)
                            .background(Palette.stage(), RoundedCornerShape(12.dp))
                            .padding(8.dp),
                    )
                }
                item { Field(stringResource(R.string.needs_you), payload.agents.waiting.toString()) }
                item { Field(stringResource(R.string.working_title), payload.agents.active.toString()) }
                item { Field(stringResource(R.string.idle_title), payload.agents.idle.toString()) }
                footer(resources.getString(if (id in model.direct) R.string.as_of_direct else R.string.as_of_relay, DateFormat.getTimeFormat(context).format(Date(payload.generatedAtMs))))
                header(resources.getString(R.string.sessions))
                if (payload.sessions.isEmpty()) {
                    item { Text(stringResource(R.string.no_sessions), Modifier.padding(16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant) }
                }
                items(payload.sessions) { s -> SessionRow(s, s.key == selected) { onPick(s.key) } }
                header(resources.getString(R.string.today))
                item { Field(stringResource(R.string.tokens), payload.today.tokens) }
                item { Field(stringResource(R.string.cost), payload.today.cost) }
                item { Field(stringResource(R.string.output_rate), payload.localizedOutputRate(resources)) }
                if (payload.today.costNote.isNotEmpty()) footer(payload.today.localizedCostNote(resources))
            }
            item {
                TextButton(onClick = { confirming = true }, Modifier.padding(8.dp)) {
                    Text(stringResource(R.string.remove), color = MaterialTheme.colorScheme.error)
                }
            }
        }
        if (confirming) {
            AlertDialog(
                onDismissRequest = { confirming = false },
                title = { Text(stringResource(R.string.remove_mac, pairing.macName)) },
                text = { Text(stringResource(if (isTablet()) R.string.remove_tablet else R.string.remove_phone)) },
                confirmButton = {
                    TextButton(onClick = {
                        confirming = false
                        model.remove(pairing)
                    }) { Text(stringResource(R.string.remove), color = MaterialTheme.colorScheme.error) }
                },
                dismissButton = { TextButton(onClick = { confirming = false }) { Text(stringResource(R.string.cancel)) } },
            )
        }
    }
}

@Composable
private fun SessionRow(s: WidgetPayload.Session, selected: Boolean, onClick: () -> Unit) {
    val (color, status) = Palette.ribbon(s.status, LocalResources.current)
    val now by rememberNow(1000)
    Column(
        Modifier
            .fillMaxWidth()
            .background(if (selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent)
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(s.project, Modifier.weight(1f), style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
            s.startedAtMs?.let {
                Text(elapsed(now - it), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
            Chip(status, color)
            Text(
                stringResource(R.string.session_summary, s.model, s.tokens, s.cost), style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

// MARK: A session

@Composable
private fun SessionPane(model: AppModel, mac: String, key: String?, back: (() -> Unit)?) {
    val payload = WidgetPayload.decode(model.pairing(mac)?.lastState)
    val s = payload?.sessions?.firstOrNull { it.key == key }
    Scaffold(topBar = { Bar(s?.project ?: "", back) }) { padding ->
        when {
            key == null -> Message(padding, stringResource(R.string.no_selected), "")
            s == null -> Message(padding, stringResource(R.string.session_ended), "")
            else -> {
                val (color, status) = Palette.ribbon(s.status, LocalResources.current)
                val now by rememberNow(1000)
                val context = LocalContext.current
                val resources = LocalResources.current
                LazyColumn(contentPadding = padding) {
                    item {
                        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text(stringResource(R.string.status), Modifier.weight(1f))
                            Chip(status, color)
                        }
                    }
                    item { Field(stringResource(R.string.model), s.model) }
                    s.startedAtMs?.let { item { Field(stringResource(R.string.running_for), elapsed(now - it)) } }
                    item { Field(stringResource(R.string.tokens), s.tokens) }
                    item { Field(stringResource(R.string.cost), s.cost) }
                    footer(resources.getString(R.string.as_of, DateFormat.getTimeFormat(context).format(Date(payload.generatedAtMs))))
                }
            }
        }
    }
}

// MARK: Parts

@Composable
private fun Bar(title: String, back: (() -> Unit)?) {
    TopAppBar(
        title = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        navigationIcon = {
            if (back != null) {
                IconButton(onClick = back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = stringResource(R.string.back)) }
            }
        },
    )
}

@Composable
fun Message(padding: androidx.compose.foundation.layout.PaddingValues, title: String, text: String, textAlign: TextAlign = TextAlign.Unspecified, action: @Composable () -> Unit = {}) {
    Column(
        Modifier.fillMaxSize().padding(padding).padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(title, style = MaterialTheme.typography.titleLarge)
        if (text.isNotEmpty()) {
            Spacer(Modifier.height(8.dp))
            Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = textAlign)
        }
        Spacer(Modifier.height(16.dp))
        action()
    }
}

@Composable
private fun Field(label: String, value: String) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp)) {
        Text(label, Modifier.weight(1f))
        Text(value, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

private fun LazyListScope.header(text: String) = item {
    Text(
        text, Modifier.padding(start = 16.dp, end = 16.dp, top = 20.dp, bottom = 8.dp),
        style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary,
    )
}

private fun LazyListScope.footer(text: String) = item {
    Text(
        text, Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun Chip(text: String, color: Color) {
    Text(
        text,
        Modifier.background(color, RoundedCornerShape(50)).padding(horizontal = 6.dp, vertical = 1.dp),
        color = Color.White, style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.Bold,
    )
}

/** The Mac's cat sprite, scaled by whole pixels and centred (macos/Shared's CatView). */
@Composable
private fun CatView(cat: WidgetPayload.Cat, modifier: Modifier) {
    val description = cat.description(LocalResources.current)
    Box(modifier.semantics { contentDescription = description }) {
        Canvas(Modifier.fillMaxSize()) {
            val w = cat.width.coerceAtLeast(1).toFloat()
            val h = cat.height.coerceAtLeast(1).toFloat()
            val scale = floor(minOf(size.width / w, size.height / h)).coerceAtLeast(1f)
            val ox = floor((size.width - w * scale) / 2)
            val oy = floor((size.height - h * scale) / 2)
            cat.rows.forEachIndexed { y, row ->
                row.forEachIndexed { x, ch ->
                    val c = cat.palette[ch.toString()]?.takeIf { it.size == 4 } ?: return@forEachIndexed
                    drawRect(
                        Color(c[0], c[1], c[2], c[3]),
                        topLeft = Offset(ox + x * scale, oy + y * scale), size = Size(scale, scale),
                    )
                }
            }
        }
    }
}

/** macos/Shared's widget palette. */
object Palette {
    val need = Color(0xFFD92D6A)
    val work = Color(0xFF2F6FE0)
    val idle = Color(0xFF6E737B)

    @Composable
    fun stage() = if (isSystemInDarkTheme()) Color(0xFF30263A) else Color(0xFFF1EAF3)

    fun ribbon(status: String, resources: android.content.res.Resources) = when (status) {
        "Waiting" -> need to resources.getString(R.string.needs_lower)
        "Active" -> work to resources.getString(R.string.working_lower)
        "Completed" -> idle to resources.getString(R.string.completed_lower)
        "Failed" -> idle to resources.getString(R.string.failed_lower)
        else -> idle to resources.getString(R.string.idle_lower)
    }
}

/** The clock, ticking every [periodMs]. */
@Composable
fun rememberNow(periodMs: Long) = produceState(System.currentTimeMillis()) {
    while (true) {
        delay(periodMs)
        value = System.currentTimeMillis()
    }
}

/** 4:05 or 1:04:05. */
fun elapsed(ms: Long): String {
    val s = (ms / 1000).coerceAtLeast(0)
    return if (s >= 3600) "%d:%02d:%02d".format(s / 3600, s % 3600 / 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
}

@Composable
fun isTablet() = LocalConfiguration.current.smallestScreenWidthDp >= 600
