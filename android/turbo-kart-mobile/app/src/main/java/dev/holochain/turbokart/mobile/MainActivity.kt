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
 * No actual racing UI or real-time Iroh/MoQ transport yet.
 */
class MainActivity : Activity() {
    private data class LobbyChoice(val id: String, val label: String)

    private val worker = Executors.newSingleThreadExecutor()
    private lateinit var linkerUrl: EditText
    private lateinit var linkerKey: EditText
    private lateinit var startButton: Button
    private lateinit var listButton: Button
    private lateinit var lobbySelector: Spinner
    private lateinit var inspectButton: Button
    private lateinit var joinButton: Button
    private lateinit var leaveButton: Button
    private lateinit var log: TextView

    private var started = false
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
        lobbySelector = Spinner(this)
        column.addView(startButton)
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
        listButton.isEnabled = !busy && started
        lobbySelector.isEnabled = !busy && started && lobbyChoices.isNotEmpty()
        inspectButton.isEnabled = !busy && started && lobbyChoices.isNotEmpty()
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
