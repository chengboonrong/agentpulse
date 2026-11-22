@file:OptIn(ExperimentalMaterial3Api::class)

package io.github.chengboonrong.agentpulse

import android.Manifest
import android.content.ClipboardManager
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
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
import androidx.compose.ui.res.stringResource
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import io.github.chengboonrong.agentpulse.link.Pairing
import io.github.chengboonrong.agentpulse.link.PairingLink
import io.github.chengboonrong.agentpulse.link.PhoneException
import java.net.URI
import kotlinx.coroutines.delay

/**
 * Pairing (docs/mobile.md, "Pairing"): scan the Mac's code or paste its link, confirm the
 * Mac and relay, show the six-digit code, wait for the Mac's first state.
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
                title = { Text(stringResource(R.string.pair_mac)) },
                navigationIcon = { TextButton(onClick = cancel) { Text(stringResource(R.string.cancel)) } },
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
            // An opened link is read first: the camera, and its permission dialog, only if it's bad.
            if (request.step == null && request.link != null && request.error == null) return@Column
            when (val s = request.step ?: PairStep.Scan) {
                PairStep.Scan -> Scan(request.error, take)
                is PairStep.Confirm -> Confirm(s.link, request.error, model.sendingHello) { model.pair(s.raw, device) }
                is PairStep.Code -> Code(model, s.code, s.pairing)
            }
        }
    }
}

/** The camera when there is one and it's allowed, else the link pasted; either way on request. */
@Composable
private fun Scan(error: String?, take: (String) -> Unit) {
    val context = LocalContext.current
    // The back camera, which CameraX opens: a front camera alone isn't for scanning a screen.
    val camera = remember { context.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA) }
    fun allowed() = ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
    var granted by remember { mutableStateOf(allowed()) }
    var pasting by rememberSaveable { mutableStateOf(!camera) }
    // Asked once on its own; again only from "Scan the code instead".
    var asked by rememberSaveable { mutableStateOf(false) }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        granted = it
        if (!it) pasting = true
    }
    fun scan() {
        pasting = false
        if (!granted) {
            asked = true
            ask.launch(Manifest.permission.CAMERA)
        }
    }
    LaunchedEffect(Unit) { if (!pasting && !granted && !asked) scan() }
    // Back from Settings, say.
    LifecycleResumeEffect(Unit) {
        granted = allowed()
        onPauseOrDispose {}
    }

    if (pasting) {
        Paste(
            error, take,
            if (camera && granted) stringResource(R.string.paste_hint)
            else stringResource(R.string.no_camera_hint),
        )
        if (camera) TextButton(onClick = ::scan) { Text(stringResource(R.string.scan_instead)) }
        return
    }
    Box(
        Modifier
            // 280 dp of the height left for the bars, the hint and the button: on a phone in
            // landscape they stay in view.
            .widthIn(max = (LocalConfiguration.current.screenHeightDp - 280).coerceIn(160, 480).dp)
            .fillMaxWidth()
            .aspectRatio(1f)
            .clip(RoundedCornerShape(16.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant),
    ) {
        if (granted) Scanner(take, Modifier.fillMaxSize())
    }
    Text(
        error ?: stringResource(R.string.scan_hint),
        color = if (error != null) MaterialTheme.colorScheme.error else Color.Unspecified,
        textAlign = TextAlign.Center,
    )
    TextButton(onClick = { pasting = true }) { Text(stringResource(R.string.paste_instead)) }
}

@Composable
private fun Paste(error: String?, take: (String) -> Unit, hint: String) {
    var pasted by rememberSaveable { mutableStateOf("") }
    val context = LocalContext.current
    OutlinedTextField(
        pasted, { pasted = it },
        Modifier.fillMaxWidth(),
        label = { Text(stringResource(R.string.pairing_link)) },
        placeholder = { Text(stringResource(R.string.pairing_url)) },
        textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false),
        minLines = 3,
        maxLines = 6,
    )
    Text(
        hint,
        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    OutlinedButton(onClick = {
        val clip = (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).primaryClip
        clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(context)?.let { pasted = it.toString() }
    }) { Text(stringResource(R.string.paste)) }
    error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    Button(onClick = { take(pasted) }, enabled = pasted.isNotBlank()) { Text(stringResource(R.string.continue_button)) }
}

@Composable
private fun Confirm(link: PairingLink, error: String?, working: Boolean, pair: () -> Unit) {
    Text(
        stringResource(R.string.pair_with, link.macName), style = MaterialTheme.typography.headlineSmall,
        fontWeight = FontWeight.Bold, textAlign = TextAlign.Center,
    )
    Row(
        Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(12.dp))
            .padding(16.dp),
    ) {
        Text(stringResource(R.string.relay), Modifier.weight(1f))
        Text(runCatching { URI(link.relay).host }.getOrNull() ?: link.relay, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
    Text(
        stringResource(if (isTablet()) R.string.pair_tablet_hint else R.string.pair_phone_hint),
        style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
        textAlign = TextAlign.Center,
    )
    error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    Button(onClick = pair, enabled = !working, modifier = Modifier.fillMaxWidth()) { Text(stringResource(R.string.pair)) }
}

@Composable
private fun Code(model: AppModel, code: String, pairing: Pairing) {
    val declined = stringResource(R.string.declined)
    var error by remember { mutableStateOf<String?>(null) }
    Text(stringResource(R.string.check_code), style = MaterialTheme.typography.titleMedium)
    Text(
        stringResource(R.string.pairing_code, code.take(3), code.takeLast(3)),
        Modifier.semantics { contentDescription = code.toList().joinToString(" ") },
        fontSize = 56.sp, fontWeight = FontWeight.Bold,
    )
    Text(stringResource(R.string.confirm_mac), color = MaterialTheme.colorScheme.onSurfaceVariant)
    error?.let { Text(it, color = MaterialTheme.colorScheme.error) } ?: run {
        CircularProgressIndicator()
        Text(stringResource(R.string.waiting_for, pairing.macName), color = MaterialTheme.colorScheme.onSurfaceVariant)
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
                    if (e.reason == PhoneException.Reason.REMOVED) error = declined
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
