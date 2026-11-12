package dev.heka.ghost.call.link

import android.app.Activity
import android.app.KeyguardManager
import android.content.Context
import android.content.Intent
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.WindowManager
import android.widget.LinearLayout
import android.widget.TextView
import java.lang.ref.WeakReference

/**
 * The incoming call, full screen and over the lock screen (the ring's full-screen intent).
 * Native and tiny, so it shows at once with the app's JavaScript not running. It says which
 * agent is calling and nothing of what about. Answer stops the ringing, tells the desk, and
 * opens the app's call (after the phone is unlocked: a ring opens the microphone, so the lock
 * stays the operator's). Decline tells the desk "not now". A ring never answers itself.
 */
class RingActivity : Activity() {
  private var ringId = ""
  private var agent = "ghost"
  private var key = ""

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    LinkHub.init(this)
    if (Build.VERSION.SDK_INT >= 27) {
      setShowWhenLocked(true)
      setTurnScreenOn(true)
    } else {
      @Suppress("DEPRECATION")
      window.addFlags(WindowManager.LayoutParams.FLAG_SHOW_WHEN_LOCKED or WindowManager.LayoutParams.FLAG_TURN_SCREEN_ON)
    }
    read(intent)
    if (intent.getBooleanExtra(EXTRA_ANSWER, false)) return answer()
    setContentView(layout(intent.getStringExtra(EXTRA_NAME) ?: Notes.agentName(agent)))
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    read(intent)
    if (intent.getBooleanExtra(EXTRA_ANSWER, false)) answer()
  }

  override fun onDestroy() {
    if (live[ringId]?.get() === this) live.remove(ringId)
    super.onDestroy()
  }

  private fun read(i: Intent) {
    ringId = i.getStringExtra(EXTRA_RING) ?: ""
    agent = i.getStringExtra(EXTRA_AGENT) ?: "ghost"
    key = i.getStringExtra(EXTRA_KEY) ?: ""
    live[ringId] = WeakReference(this)
  }

  private fun answer() {
    live.remove(ringId) // the ring stopping must not close this screen before the unlock is asked
    LinkHub.answerRing(ringId) {}
    val call = Intent(Intent.ACTION_VIEW, Uri.parse("ghost://answer?ring=${Uri.encode(ringId)}&agent=${Uri.encode(agent)}&item=${Uri.encode(key)}"))
      .setPackage(packageName)
      .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
    val km = getSystemService(KeyguardManager::class.java)
    if (km != null && km.isKeyguardLocked && Build.VERSION.SDK_INT >= 26) {
      km.requestDismissKeyguard(this, object : KeyguardManager.KeyguardDismissCallback() {
        override fun onDismissSucceeded() {
          startActivity(call)
          finish()
        }

        override fun onDismissCancelled() {
          // Still answered: the call waits in the app for the phone to be unlocked.
          startActivity(call)
          finish()
        }
      })
    } else {
      startActivity(call)
      finish()
    }
  }

  private fun decline() {
    LinkHub.declineRing(ringId)
    finish()
  }

  private fun dp(v: Int) = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v.toFloat(), resources.displayMetrics).toInt()

  private fun layout(name: String): View {
    val root = LinearLayout(this).apply {
      orientation = LinearLayout.VERTICAL
      gravity = Gravity.CENTER_HORIZONTAL
      setBackgroundColor(Color.parseColor("#0f0f11"))
      setPadding(dp(24), dp(96), dp(24), dp(64))
    }
    root.addView(TextView(this).apply {
      text = name
      setTextColor(Color.parseColor("#ededef"))
      setTextSize(TypedValue.COMPLEX_UNIT_SP, 34f)
      typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
      gravity = Gravity.CENTER
    })
    root.addView(TextView(this).apply {
      text = "is calling through Ghost"
      setTextColor(Color.parseColor("#a1a1aa"))
      setTextSize(TypedValue.COMPLEX_UNIT_SP, 18f)
      gravity = Gravity.CENTER
      setPadding(0, dp(8), 0, 0)
    })
    root.addView(View(this), LinearLayout.LayoutParams(0, 0, 1f))
    val row = LinearLayout(this).apply {
      orientation = LinearLayout.HORIZONTAL
      gravity = Gravity.CENTER
    }
    row.addView(button("Decline", "#cf3030") { decline() }, LinearLayout.LayoutParams(0, dp(80), 1f).apply { marginEnd = dp(12) })
    row.addView(button("Answer", "#15803d") { answer() }, LinearLayout.LayoutParams(0, dp(80), 1f).apply { marginStart = dp(12) })
    root.addView(row, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT))
    return root
  }

  private fun button(label: String, color: String, onTap: () -> Unit) = TextView(this).apply {
    text = label
    gravity = Gravity.CENTER
    setTextColor(Color.WHITE)
    setTextSize(TypedValue.COMPLEX_UNIT_SP, 20f)
    typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
    background = GradientDrawable().apply {
      cornerRadius = dp(40).toFloat()
      setColor(Color.parseColor(color))
    }
    isClickable = true
    isFocusable = true
    contentDescription = label
    setOnClickListener { onTap() }
  }

  companion object {
    private const val EXTRA_RING = "ring"
    private const val EXTRA_AGENT = "agent"
    private const val EXTRA_KEY = "key"
    private const val EXTRA_NAME = "name"
    private const val EXTRA_ANSWER = "answer"
    private val live = HashMap<String, WeakReference<RingActivity>>()

    fun intent(ctx: Context, call: RingCall, name: String, answer: Boolean): Intent =
      Intent(ctx, RingActivity::class.java)
        // One per ring and per button, so a PendingIntent never stands in for another.
        .setData(Uri.parse("ghost-ring://${Uri.encode(call.ringId)}/${if (answer) "answer" else "show"}"))
        .putExtra(EXTRA_RING, call.ringId)
        .putExtra(EXTRA_AGENT, call.agent)
        .putExtra(EXTRA_KEY, call.key)
        .putExtra(EXTRA_NAME, name)
        .putExtra(EXTRA_ANSWER, answer)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_USER_ACTION)

    /** The ring is over: close its screen if it is up. */
    fun finish(ringId: String) {
      Handler(Looper.getMainLooper()).post { live.remove(ringId)?.get()?.finish() }
    }
  }
}
