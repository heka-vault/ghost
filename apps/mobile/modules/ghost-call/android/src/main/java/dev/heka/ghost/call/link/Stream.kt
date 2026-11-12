package dev.heka.ghost.call.link

import org.json.JSONArray
import org.json.JSONObject

/** One Server-Sent Event as the desk writes it: `event:`, `id:` when numbered, `data:`. */
data class SseEvent(val event: String?, val id: String?, val data: String)

/**
 * Server-Sent Events, a line at a time (the WHATWG rules the desk's frames need): `field:
 * value` lines, a single space after the colon dropped, `data` lines joined by newlines, a
 * blank line dispatching, `:` lines ignored. Fed by the stream reader; pure, so it is tested
 * off the phone.
 */
class SseParser {
  private var event: String? = null
  private var id: String? = null
  private val data = StringBuilder()
  private var hasData = false

  /** One line without its line ending; an event when the line ends one. */
  fun line(raw: String): SseEvent? {
    val line = raw.removeSuffix("\r")
    if (line.isEmpty()) {
      val out = if (hasData) SseEvent(event, id, data.toString()) else null
      event = null
      id = null
      data.setLength(0)
      hasData = false
      return out
    }
    if (line.startsWith(":")) return null
    val colon = line.indexOf(':')
    val field = if (colon < 0) line else line.substring(0, colon)
    var value = if (colon < 0) "" else line.substring(colon + 1)
    if (value.startsWith(" ")) value = value.substring(1)
    when (field) {
      "event" -> event = value
      "id" -> id = value
      "data" -> {
        if (hasData) data.append('\n')
        data.append(value)
        hasData = true
      }
    }
    return null
  }
}

/** A question as the stream says it (QuestionView), with the fields the phone acts on. */
data class Question(
  val key: String,
  val kind: String,
  val askedBy: String,
  val source: String,
  val question: String,
  val detail: String?,
  val choices: List<String>,
  val choicesOnly: Boolean,
  val urgency: String,
  val state: String,
  val changeId: String?,
  val askedAt: String,
  /** The view as the desk sent it, handed to the app unchanged. */
  val raw: JSONObject,
) {
  val open get() = state == "open"
  val isChange get() = kind == "change"

  companion object {
    fun from(o: JSONObject): Question? {
      val key = o.optString("key").takeIf { it.isNotEmpty() } ?: return null
      return Question(
        key = key,
        kind = o.optString("kind", "question"),
        askedBy = o.optString("asked_by", "Ghost"),
        source = o.optString("source"),
        question = o.optString("question"),
        detail = o.optStringOrNull("detail"),
        choices = o.optJSONArray("choices").strings(),
        choicesOnly = o.optBoolean("choices_only", false),
        urgency = o.optString("urgency", "notify"),
        state = o.optString("state", "open"),
        changeId = o.optStringOrNull("change_id"),
        askedAt = o.optString("asked_at"),
        raw = o,
      )
    }
  }
}

/** One waiting-list item as the stream says it (WaitingItemView). */
data class Item(
  val key: String,
  val source: String,
  val title: String,
  val state: String,
  val firstSeen: String,
  val question: Question?,
  val raw: JSONObject,
) {
  companion object {
    fun from(o: JSONObject): Item? {
      val key = o.optString("key").takeIf { it.isNotEmpty() } ?: return null
      return Item(
        key = key,
        source = o.optString("source"),
        title = o.optString("title"),
        state = o.optString("state", "new"),
        firstSeen = o.optString("first_seen"),
        question = o.optJSONObject("question")?.let(Question::from),
        raw = o,
      )
    }
  }
}

/** What the desk says on the live stream (docs/phone-protocol.md, "Message types"). */
sealed class Msg {
  /** The cursor of a numbered message, or where a heartbeat or snapshot stands. */
  open val cursor: String? get() = null

  data class Hello(val epoch: String, val resumed: Boolean, val heartbeatSecs: Int) : Msg()
  data class Snapshot(override val cursor: String, val items: List<Item>, val questions: List<Question>) : Msg()
  data class ItemChange(val change: String, val key: String, val item: Item?, override val cursor: String?) : Msg()
  data class QuestionChange(val question: Question, override val cursor: String?) : Msg()
  data class Alert(val alertId: String, val keys: List<String>, val titles: List<String>, override val cursor: String?) : Msg()
  data class Ring(val ringId: String, val key: String, val agent: String, val title: String, val question: String?, val ringSecs: Int, override val cursor: String?) : Msg()
  data class RingOver(val ringId: String, val why: String, override val cursor: String?) : Msg()
  data class Heartbeat(override val cursor: String?, val at: String?) : Msg()
  /** A type this app doesn't know yet: kept for the cursor, otherwise ignored. */
  data class Other(val type: String, override val cursor: String?) : Msg()

  companion object {
    fun parse(text: String): Msg? {
      val o = try {
        JSONObject(text)
      } catch (e: Exception) {
        return null
      }
      val cursor = o.optStringOrNull("cursor")
      return when (val type = o.optString("type")) {
        "hello" -> Hello(o.optString("epoch"), o.optBoolean("resumed", false), o.optInt("heartbeat_secs", 15))
        "snapshot" -> Snapshot(
          cursor ?: return null,
          o.optJSONArray("items").objects().mapNotNull(Item::from),
          o.optJSONArray("questions").objects().mapNotNull(Question::from),
        )
        "item" -> {
          val key = o.optString("key").takeIf { it.isNotEmpty() } ?: return null
          ItemChange(o.optString("change"), key, o.optJSONObject("item")?.let(Item::from), cursor)
        }
        "question" -> QuestionChange(o.optJSONObject("question")?.let(Question::from) ?: return null, cursor)
        "alert" -> Alert(
          o.optString("alert_id").takeIf { it.isNotEmpty() } ?: return null,
          o.optJSONArray("keys").strings(),
          o.optJSONArray("titles").strings(),
          cursor,
        )
        "ring" -> Ring(
          o.optString("ring_id").takeIf { it.isNotEmpty() } ?: return null,
          o.optString("key"),
          o.optString("agent", "ghost").ifEmpty { "ghost" },
          o.optString("title"),
          o.optStringOrNull("question"),
          o.optInt("ring_secs", 30),
          cursor,
        )
        "ring_over" -> RingOver(o.optString("ring_id"), o.optString("why"), cursor)
        "heartbeat" -> Heartbeat(cursor, o.optStringOrNull("at"))
        "" -> null
        else -> Other(type, cursor)
      }
    }
  }
}

/** What became of one event's `data`. */
sealed class Decoded {
  data class Ok(val msg: Msg) : Decoded()
  /** The signature didn't check: drop the stream and open a new one. */
  object BadSignature : Decoded()
  /** Signed, but not something this app reads. */
  object Unreadable : Decoded()
}

/**
 * `data` is `{"msg": "<message as JSON text>", "sig": "<signature>"}`: the signature is checked
 * over `msg` exactly as received, before `msg` is parsed.
 */
fun decode(data: String, secret: ByteArray, nonce: String): Decoded {
  val outer = try {
    JSONObject(data)
  } catch (e: Exception) {
    return Decoded.BadSignature
  }
  val msg = outer.optString("msg", "")
  val sig = outer.optString("sig", "")
  if (msg.isEmpty() || sig.isEmpty() || !Sign.verifyEvent(secret, nonce, msg, sig)) return Decoded.BadSignature
  return Msg.parse(msg)?.let { Decoded.Ok(it) } ?: Decoded.Unreadable
}

internal fun JSONObject.optStringOrNull(name: String): String? =
  if (!has(name) || isNull(name)) null else optString(name)

internal fun JSONArray?.strings(): List<String> =
  if (this == null) emptyList() else (0 until length()).mapNotNull { if (isNull(it)) null else optString(it) }

internal fun JSONArray?.objects(): List<JSONObject> =
  if (this == null) emptyList() else (0 until length()).mapNotNull { optJSONObject(it) }
