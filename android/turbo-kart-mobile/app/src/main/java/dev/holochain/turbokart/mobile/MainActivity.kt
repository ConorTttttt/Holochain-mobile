package dev.holochain.turbokart.mobile

import android.app.Activity
import android.os.Build
import android.os.Bundle
import android.view.WindowInsets
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.Spinner
import android.widget.TextView
import org.json.JSONObject
import java.io.File
import java.util.concurrent.Executors

/**
 * Android multiplayer control-plane prototype: H2HC/Wasmi install/resume,
 * discover, inspect, join and leave public desktop lobbies.
 * Includes persistent Iroh endpoint identity, signed directory presence and
 * read-only locked race admission validation. A separate opt-in MoQ mesh
 * diagnostic can authenticate peer handshakes, but has no race physics,
 * race controller, or renderer.
 */
class MainActivity : Activity() {
    private data class LobbyChoice(val id: String, val label: String)

    private val worker = Executors.newSingleThreadExecutor()
    private lateinit var linkerUrl: EditText
    private lateinit var linkerKey: EditText
    private lateinit var startButton: Button
    private lateinit var listButton: Button
    private lateinit var irohButton: Button
    private lateinit var publishPresenceButton: Button
    private lateinit var verifyPresenceButton: Button
    private lateinit var lobbySelector: Spinner
    private lateinit var inspectButton: Button
    private lateinit var inspectRaceButton: Button
    private lateinit var verifyRaceButton: Button
    private lateinit var connectRaceButton: Button
    private lateinit var raceStatusButton: Button
    private lateinit var sendTestKartButton: Button
    private lateinit var streamKartButton: Button
    private lateinit var disconnectRaceButton: Button
    private lateinit var joinButton: Button
    private lateinit var leaveButton: Button
    private lateinit var log: TextView

    private var started = false
    private var irohStarted = false
    private var raceActive = false
    private var raceAuthenticated = false
    private var streamRequested = false
    private var streamRunning = false
    private var busy = false
    private var agentId: String? = null
    private var joinedLobbyId: String? = null
    private var lobbyChoices: List<LobbyChoice> = emptyList()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(24, 24, 24, 24)
        }
        val scroll = ScrollView(this).apply {
            clipToPadding = false
            setOnApplyWindowInsetsListener { view, insets ->
                val top: Int
                val bottom: Int
                if (Build.VERSION.SDK_INT >= 30) {
                    top = insets.getInsets(WindowInsets.Type.statusBars()).top
                    bottom = insets.getInsets(WindowInsets.Type.navigationBars()).bottom
                } else {
                    top = insets.systemWindowInsetTop
                    bottom = insets.systemWindowInsetBottom
                }
                view.setPadding(0, top, 0, bottom)
                insets
            }
            addView(column)
        }
        setContentView(scroll)

        column.addView(TextView(this).apply {
            text = "Turbo Kart • Android H2HC / Wasmi"
            textSize = 22f
        })
        column.addView(TextView(this).apply {
            text = "Public lobby test. Keep the desktop linker running and use adb reverse tcp:8090 tcp:8090."
        })

        linkerUrl = EditText(this).apply {
            setSingleLine(true)
            hint = "Linker WebSocket URL"
            setText("ws://127.0.0.1:8090/conductor/v1")
        }
        linkerKey = EditText(this).apply {
            setSingleLine(true)
            hint = "Padded base64 linker public key"
            setText("OC6JeMhmkekjeMjZZMpMzq2SVNUxkX/MSo2zWLIxufQ=")
        }
        column.addView(linkerUrl)
        column.addView(linkerKey)

        startButton = Button(this).apply {
            text = "Start / Resume Holochain"
            setOnClickListener { startConductor() }
        }
        listButton = Button(this).apply {
            text = "Discover desktop lobbies"
            setOnClickListener {
                submit(
                    "Listing desktop lobbies…",
                    onSuccess = { updateLobbies(it) }
                ) {
                    NativeBridge.listOpenLobbies(10)
                }
            }
        }
        irohButton = Button(this).apply {
            text = "Start / Resume Iroh Endpoint (Test)"
            setOnClickListener { startIrohEndpoint() }
        }
        lobbySelector = Spinner(this)
        column.addView(startButton)
        column.addView(irohButton)
        column.addView(TextView(this).apply {
            text = "Iroh identity persists across restarts. Next, publish a signed " +
                "EndpointId-to-player binding through the existing Holochain directory. " +
                "The manual binding expires after 120 seconds; publish again to refresh."
        })
        publishPresenceButton = Button(this).apply {
            text = "Publish signed Iroh presence"
            setOnClickListener { publishIrohPresence() }
        }
        verifyPresenceButton = Button(this).apply {
            text = "Verify directory Iroh bindings"
            setOnClickListener {
                submit("Reading original Holochain directory bindings…") {
                    NativeBridge.recentSignedBindings()
                }
            }
        }
        column.addView(publishPresenceButton)
        column.addView(verifyPresenceButton)
        column.addView(listButton)
        column.addView(TextView(this).apply { text = "Select a desktop lobby:" })
        column.addView(lobbySelector)
        updateLobbiesAdapter()

        inspectButton = Button(this).apply {
            text = "View selected lobby"
            setOnClickListener {
                val choice = selectedLobby() ?: return@setOnClickListener
                submit(
                    "Reading ${choice.label}…",
                    onSuccess = { inspectMembership(choice.id, it) }
                ) {
                    NativeBridge.getLobby(choice.id)
                }
            }
        }
        inspectRaceButton = Button(this).apply {
            text = "Check locked race (read-only)"
            setOnClickListener {
                // A started race no longer appears among open lobby results.
                // Prefer our remembered joined lobby while this process runs.
                val lobby = joinedLobbyId ?: selectedLobby()?.id ?: return@setOnClickListener
                submit("Reading desktop race admission snapshot (not authenticated yet)…") {
                    NativeBridge.getLockedRace(lobby)
                }
            }
        }
        verifyRaceButton = Button(this).apply {
            text = "Verify signed race admission (no connection)"
            setOnClickListener {
                val lobby = joinedLobbyId ?: selectedLobby()?.id ?: return@setOnClickListener
                verifyRaceAdmission(lobby)
            }
        }
        connectRaceButton = Button(this).apply {
            text = "Connect Iroh/MoQ (test)"
            setOnClickListener {
                val lobby = joinedLobbyId ?: selectedLobby()?.id ?: return@setOnClickListener
                submit(
                    "Opening frozen-roster Iroh/MoQ mesh; waiting for desktop peer…",
                    onSuccess = {
                        raceActive = true
                        raceAuthenticated = it.optJSONObject("result")
                            ?.optBoolean("authenticated_moq_peer_session") == true
                        streamRequested = false
                        streamRunning = false
                    }
                ) {
                    val bundle = JSONObject(NativeBridge.lockedAdmissionBundle(lobby))
                    check(bundle.optBoolean("ok")) {
                        bundle.optString("error", "No locked race or missing signed binding")
                    }
                    IrohBridge.connectLockedRace(bundle.getJSONObject("result").toString())
                }
            }
        }
        raceStatusButton = Button(this).apply {
            text = "Check Iroh/MoQ connection status"
            setOnClickListener {
                submit(
                    "Checking remote Iroh identity and frozen race admission digest…",
                    onSuccess = {
                        raceActive = true
                        val status = it.optJSONObject("result")
                        raceAuthenticated = status
                            ?.optBoolean("authenticated_moq_peer_session") == true
                        val stream = status?.optJSONObject("diagnostic_wp3_stream")
                        streamRequested = stream?.optBoolean("requested") == true
                        streamRunning = stream?.optBoolean("running") == true
                    }
                ) {
                    IrohBridge.raceStatus()
                }
            }
        }
        sendTestKartButton = Button(this).apply {
            text = "Send ONE test kart packet (42 bytes)"
            setOnClickListener {
                submit("Sending one synthetic WP3 packet via authenticated MoQ…") {
                    IrohBridge.sendTestKart()
                }
            }
        }
        streamKartButton = Button(this).apply {
            text = "Send 20 Hz kart test (5s / 100 frames)"
            setOnClickListener {
                submit(
                    "Starting bounded 20 Hz synthetic WP3 stream (NOT race physics)…",
                    onSuccess = {
                        streamRequested = true
                        streamRunning = true
                    }
                ) {
                    IrohBridge.startTestStream()
                }
            }
        }
        disconnectRaceButton = Button(this).apply {
            text = "Disconnect MoQ test (keep identity)"
            setOnClickListener {
                submit(
                    "Closing race-only MoQ tracks; preserving Iroh and Holochain identities…",
                    onSuccess = {
                        raceActive = false
                        raceAuthenticated = false
                        streamRequested = false
                        streamRunning = false
                    }
                ) {
                    IrohBridge.disconnectRace()
                }
            }
        }
        joinButton = Button(this).apply {
            text = "Join as Zippy"
            setOnClickListener {
                val choice = selectedLobby() ?: return@setOnClickListener
                submit(
                    "Joining ${choice.label} as Zippy…",
                    onSuccess = { joinedLobbyId = choice.id }
                ) {
                    NativeBridge.joinLobby(choice.id, "zippy")
                }
            }
        }
        leaveButton = Button(this).apply {
            text = "Leave joined lobby"
            setOnClickListener {
                val id = joinedLobbyId ?: return@setOnClickListener
                submit(
                    "Leaving lobby…",
                    onSuccess = { joinedLobbyId = null }
                ) {
                    NativeBridge.leaveLobby(id)
                }
            }
        }
        column.addView(inspectButton)
        column.addView(inspectRaceButton)
        column.addView(verifyRaceButton)
        column.addView(connectRaceButton)
        column.addView(raceStatusButton)
        column.addView(sendTestKartButton)
        column.addView(streamKartButton)
        column.addView(disconnectRaceButton)
        column.addView(joinButton)
        column.addView(leaveButton)

        log = TextView(this).apply {
            text = "Ready. Start Holochain, discover a desktop public lobby, select it, then join as Zippy."
            setTextIsSelectable(true)
        }
        column.addView(log)
        updateButtons()
    }

    private fun selectedLobby(): LobbyChoice? =
        lobbyChoices.getOrNull(lobbySelector.selectedItemPosition)

    private fun updateLobbiesAdapter() {
        val labels = if (lobbyChoices.isEmpty()) {
            listOf("No lobby selected — tap Discover")
        } else {
            lobbyChoices.map { it.label }
        }
        lobbySelector.adapter = ArrayAdapter(
            this,
            android.R.layout.simple_spinner_dropdown_item,
            labels
        )
    }

    private fun updateLobbies(reply: JSONObject) {
        val items = reply.optJSONObject("result")?.optJSONArray("lobbies")
        val discovered = mutableListOf<LobbyChoice>()
        if (items != null) {
            for (index in 0 until items.length()) {
                val item = items.optJSONObject(index) ?: continue
                val id = item.optString("id")
                val lobby = item.optJSONObject("lobby") ?: continue
                if (id.isBlank() || lobby.optString("access") != "Public") continue
                val name = lobby.optString("name", "Unnamed lobby")
                val track = lobby.optString("track_id", "unknown track")
                val size = lobby.optInt("max_players", 0)
                discovered.add(LobbyChoice(id, "$name • $track • max $size"))
            }
        }
        lobbyChoices = discovered
        updateLobbiesAdapter()
    }

    private fun inspectMembership(id: String, reply: JSONObject) {
        val currentAgent = agentId ?: return
        val members = reply.optJSONObject("result")
            ?.optJSONObject("lobby")
            ?.optJSONArray("members") ?: return
        var memberPresent = false
        for (index in 0 until members.length()) {
            val member = members.optJSONObject(index) ?: continue
            if (member.optString("player") == currentAgent) {
                memberPresent = true
                break
            }
        }
        // On an app restart, recover the lobby membership status from the
        // actual Holochain state rather than from ephemeral Android UI state.
        if (memberPresent) {
            joinedLobbyId = id
        } else if (joinedLobbyId == id) {
            joinedLobbyId = null
        }
    }

    private fun updateButtons() {
        startButton.isEnabled = !busy && !started
        irohButton.isEnabled = !busy && started && !irohStarted
        publishPresenceButton.isEnabled = !busy && started && irohStarted
        verifyPresenceButton.isEnabled = !busy && started
        listButton.isEnabled = !busy && started
        lobbySelector.isEnabled = !busy && started && lobbyChoices.isNotEmpty()
        inspectButton.isEnabled = !busy && started && lobbyChoices.isNotEmpty()
        inspectRaceButton.isEnabled = !busy && started && (joinedLobbyId != null || lobbyChoices.isNotEmpty())
        verifyRaceButton.isEnabled = !busy && started && irohStarted &&
            (joinedLobbyId != null || lobbyChoices.isNotEmpty())
        connectRaceButton.isEnabled = !busy && started && irohStarted && !raceActive &&
            (joinedLobbyId != null || lobbyChoices.isNotEmpty())
        raceStatusButton.isEnabled = !busy && irohStarted
        sendTestKartButton.isEnabled = !busy && raceActive && raceAuthenticated && !streamRunning
        streamKartButton.isEnabled = !busy && raceActive && raceAuthenticated && !streamRequested
        disconnectRaceButton.isEnabled = !busy && raceActive
        joinButton.isEnabled = !busy && started && lobbyChoices.isNotEmpty() && joinedLobbyId == null
        leaveButton.isEnabled = !busy && started && joinedLobbyId != null
    }

    private fun copyHapp(name: String): String {
        val dir = File(filesDir, "bundles")
        check(dir.exists() || dir.mkdirs()) { "Cannot create app-private bundle directory" }
        val file = File(dir, name)
        val tmp = File(dir, "$name.tmp")
        assets.open(name).use { source ->
            tmp.outputStream().use { destination -> source.copyTo(destination) }
        }
        check(tmp.renameTo(file)) { "Cannot stage $name bundle" }
        return file.absolutePath
    }

    private fun startConductor() {
        val url = linkerUrl.text.toString().trim()
        val key = linkerKey.text.toString().trim()
        submit(
            "Starting native Wasmi conductor…",
            onSuccess = { reply ->
                agentId = reply.optJSONObject("result")?.optString("agent")
                started = true
            }
        ) {
            val secret = PassphraseStore(applicationContext).getOrCreate()
            try {
                NativeBridge.start(
                    File(filesDir, "conductor").absolutePath,
                    url,
                    key,
                    copyHapp("gaming_platform.happ"),
                    copyHapp("turbo_kart.happ"),
                    copyHapp("expected-dna-hashes.json"),
                    secret
                )
            } finally {
                secret.fill(0)
            }
        }
    }

    /**
     * The Iroh identity is independent of Lair and securely persisted in
     * Android Keystore-protected app data. No signed directory advert yet.
     */
    private fun startIrohEndpoint() {
        submit(
            "Starting original desktop Iroh/MoQ transport on Android…",
            onSuccess = { irohStarted = true }
        ) {
            val seed = IrohIdentityStore(applicationContext).getOrCreate()
            try {
                IrohBridge.startEndpoint(seed)
            } finally {
                seed.fill(0)
            }
        }
    }

    /**
     * Signs using the persistent Iroh key inside the Iroh JNI library, then
     * publishes using the existing Holochain agent in the Wasmi JNI host.
     * The Ed25519 private key is never sent to Kotlin or to Holochain.
     */
    private fun publishIrohPresence() {
        val agent = agentId?.takeIf { it.isNotBlank() }
        if (agent == null) {
            log.text = "Start Holochain and verify the restored agent first."
            return
        }
        submit("Signing Iroh endpoint and publishing to original gaming directory…") {
            val signedResult = JSONObject(IrohBridge.signPresence(agent))
            check(signedResult.optBoolean("ok")) {
                signedResult.optString("error", "Could not sign Iroh endpoint binding")
            }
            val signedPresence = signedResult.getJSONObject("result")
                .getJSONObject("presence")
            NativeBridge.publishSignedPresence(signedPresence.toString())
        }
    }

    /**
     * The Wasmi JNI host reads the original locked RaceRecord and each exact
     * signed binding from Holochain. The isolated desktop-linked Iroh JNI
     * validates the roster with RaceAdmission::from_locked and checks its
     * actual persistent EndpointId. This never initiates a race connection.
     */
    private fun verifyRaceAdmission(lobbyId: String) {
        submit("Verifying frozen signed race roster and persistent Iroh endpoint…") {
            val bundle = JSONObject(NativeBridge.lockedAdmissionBundle(lobbyId))
            check(bundle.optBoolean("ok")) {
                bundle.optString("error", "Could not read locked race and signed bindings")
            }
            IrohBridge.validateLockedAdmission(bundle.getJSONObject("result").toString())
        }
    }

    /**
     * All JNI work runs off the UI thread. A successful commit is the only
     * event that updates membership; a failed join/leave leaves buttons
     * retryable and never pretends an action succeeded.
     */
    private fun submit(
        message: String,
        onSuccess: ((JSONObject) -> Unit)? = null,
        job: () -> String
    ) {
        if (busy) return
        busy = true
        updateButtons()
        log.text = message
        worker.execute {
            val result = try {
                job()
            } catch (error: Throwable) {
                JSONObject().put("ok", false)
                    .put("error", error.message ?: error.javaClass.simpleName).toString()
            }
            runOnUiThread {
                val parsed = try { JSONObject(result) } catch (_: Exception) { null }
                if (parsed?.optBoolean("ok") == true) {
                    onSuccess?.invoke(parsed)
                }
                busy = false
                updateButtons()
                log.text = parsed?.toString(2) ?: result
            }
        }
    }

    override fun onDestroy() {
        worker.shutdown()
        super.onDestroy()
    }
}
