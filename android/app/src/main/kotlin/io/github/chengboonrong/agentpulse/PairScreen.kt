@file:OptIn(ExperimentalMaterial3Api::class)

package io.github.chengboonrong.agentpulse

import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import io.github.chengboonrong.agentpulse.link.Pairing
import io.github.chengboonrong.agentpulse.link.PairingLink
import io.github.chengboonrong.agentpulse.link.PhoneException
import java.net.URI
import kotlinx.coroutines.delay

/**
 * Pairing (docs/mobile.md, "Pairing"): paste the link (scanning comes with CameraX and
 * ZXing), confirm the Mac and relay, show the six-digit code, wait for the Mac's first state.
 */
@Composable
fun PairScreen(model: AppModel, request: PairRequest) {
    val device = deviceName(LocalContext.current)
    val cancel = { model.pairRequest = null }
    BackHandler(onBack = cancel)
    val take = { raw: String -> model.take(raw, device) }

    LaunchedEffect(request) { if (request.step == null) request.link?.let(take) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Pair a Mac") },
                navigationIcon = { TextButton(onClick = cancel) { Text("Cancel") } },
            )
        },
    ) { padding ->
        Column(
            Modifier
                .padding(padding)
                .verticalScroll(rememberScrollState())
                .padding(16.dp)
                .fillMaxWidth(),
            verticalArrangement = Arrangement.spacedBy(16.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            when (val s = request.step ?: PairStep.Paste) {
                PairStep.Paste -> Paste(request.error, take)
                is PairStep.Confirm -> Confirm(s.link, request.error, model.sendingHello) { model.pair(s.raw, device) }
                is PairStep.Code -> Code(model, s.code, s.pairing)
            }
        }
    }
}

@Composable
private fun Paste(error: String?, take: (String) -> Unit) {
    var pasted by rememberSaveable { mutableStateOf("") }
    val context = LocalContext.current
    OutlinedTextField(
        pasted, { pasted = it },
        Modifier.fillMaxWidth(),
        label = { Text("Pairing link") },
        placeholder = { Text("agentpulse://pair?…") },
        textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false),
        minLines = 3,
        maxLines = 6,
    )
    Text(
        "On your Mac, copy the link under the pairing code and paste it here.",
        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    OutlinedButton(onClick = {
        val clip = (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).primaryClip
        clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(context)?.let { pasted = it.toString() }
    }) { Text("Paste") }
    error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    Button(onClick = { take(pasted) }, enabled = pasted.isNotBlank()) { Text("Continue") }
}

@Composable
private fun Confirm(link: PairingLink, error: String?, working: Boolean, pair: () -> Unit) {
    Text(
        "Pair with ${link.macName}?", style = MaterialTheme.typography.headlineSmall,
        fontWeight = FontWeight.Bold, textAlign = TextAlign.Center,
    )
    Row(
        Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(12.dp))
            .padding(16.dp),
    ) {
        Text("Relay", Modifier.weight(1f))
        Text(runCatching { URI(link.relay).host }.getOrNull() ?: link.relay, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
    Text(
        "Pair only with a Mac you own. It will send its agents' status to this ${deviceKind()}, end-to-end encrypted.",
        style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
        textAlign = TextAlign.Center,
    )
    error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    Button(onClick = pair, enabled = !working, modifier = Modifier.fillMaxWidth()) { Text("Pair") }
}

@Composable
private fun Code(model: AppModel, code: String, pairing: Pairing) {
    var error by remember { mutableStateOf<String?>(null) }
    Text("Check that your Mac shows", style = MaterialTheme.typography.titleMedium)
    Text(
        "${code.take(3)} ${code.takeLast(3)}",
        Modifier.semantics { contentDescription = code.toList().joinToString(" ") },
        fontSize = 56.sp, fontWeight = FontWeight.Bold,
    )
    Text("Then confirm on your Mac.", color = MaterialTheme.colorScheme.onSurfaceVariant)
    error?.let { Text(it, color = MaterialTheme.colorScheme.error) } ?: run {
        CircularProgressIndicator()
        Text("Waiting for ${pairing.macName}", color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
    // `GET down` every 2 s, while in the foreground, until the Mac confirms and its first state arrives.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(pairing.id) {
        if (model.offline) return@LaunchedEffect
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (error == null) {
                delay(2000)
                try {
                    val p = io { model.client.fetchDown(pairing) }
                    if (p.lastState != null) return@repeatOnLifecycle model.added(p)
                } catch (e: PhoneException) {
                    if (e.reason == PhoneException.Reason.REMOVED) error = "Your Mac declined or removed the pairing."
                } catch (_: Exception) {
                    // Offline for a moment; keep waiting.
                }
            }
        }
    }
}

/** The name the user gave this device, else its model. */
private fun deviceName(context: Context): String =
    Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)?.takeIf { it.isNotBlank() }
        ?: Build.MODEL
