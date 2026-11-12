package dev.heka.ghost.call.link

import org.json.JSONArray
import org.json.JSONObject

/** What the Android side does after a message: draw, take down, tell the desk, ring. */
sealed class Fx {
  data class Post(val note: Note) : Fx()
  data class Clear(val key: String) : Fx()
  /** Tell the desk the alert's notifications are up (`POST /seen`), if they really are. */
  data class Seen(val alertId: String, val keys: List<String>) : Fx()
  data class Ringing(val fx: RingFx) : Fx()
}

/**
 * The phone's copy of what is waiting, as the live stream says it: the items, the questions,
 * the cursor to resume from, and which keys have a notification up. Pure: `apply` turns one
 * message into effects, and the Android side carries them out.
 *
 * A notification goes up only on an `alert` (the desk's phone route, after its quiet hours,
 * batching and do-not-disturb), never for an item merely added: the desk decides when the
 * phone is the way to reach the operator. Once up, it follows the item: a new title updates
 * it, and acknowledged, snoozed, answered, declined, expired, cancelled or resolved takes it
 * down, whichever surface it happened on.
 */
class LinkState {
  var epoch: String? = null
    private set
  var cursor: String? = null
    private set
  val items = LinkedHashMap<String, Item>()
  val questions = LinkedHashMap<String, Question>()
  /** Keys with a notification up, and the alert's title for each. */
  val posted = LinkedHashMap<String, String>()
  /** Keys with an action from this phone on its way. */
  val sending = LinkedHashSet<String>()
  /** Keys the desk refused an action for, with why. */
  val errors = LinkedHashMap<String, String>()
  val rings = Rings()
  /** A session on this phone: notifications are posted silently, rings are not shown. */
  var inSession = false

  /** Apply one verified message. `now` is milliseconds on a monotonic clock. */
  fun apply(msg: Msg, now: Long): List<Fx> {
    val out = mutableListOf<Fx>()
    when (msg) {
      is Msg.Hello -> {
        if (epoch != null && msg.epoch != epoch) cursor = null // the desk restarted: a snapshot follows
        epoch = msg.epoch
      }
      is Msg.Snapshot -> {
        items.clear()
        msg.items.forEach { items[it.key] = it }
        questions.clear()
        msg.questions.forEach { questions[it.key] = it }
        for (key in posted.keys.toList()) {
          if (stillWanted(key)) out += Fx.Post(note(key, silent = true)) else out += clear(key)
        }
      }
      is Msg.ItemChange -> {
        when (msg.change) {
          "resolved" -> {
            items.remove(msg.key)
            out += clear(msg.key)
          }
          else -> msg.item?.let { item ->
            items[item.key] = item
            item.question?.let { questions[it.key] = it }
            sending.remove(item.key) // the desk's word on it: the buttons follow the item now
            errors.remove(item.key)
            if (item.key in posted) out += if (stillWanted(item.key)) Fx.Post(note(item.key, silent = true)) else clear(item.key)
          }
        }
      }
      is Msg.QuestionChange -> {
        val q = msg.question
        questions[q.key] = q
        items[q.key]?.let { items[q.key] = it.copy(question = q) }
        sending.remove(q.key)
        errors.remove(q.key)
        if (q.key in posted) out += if (q.open) Fx.Post(note(q.key, silent = true)) else clear(q.key)
      }
      is Msg.Alert -> {
        // A key the phone hasn't heard of yet is shown with the alert's title; its item or
        // question, when it arrives, fills the notification in.
        val shown = mutableListOf<String>()
        msg.keys.forEachIndexed { i, key ->
          if (key in items && !stillWanted(key)) return@forEachIndexed
          posted[key] = msg.titles.getOrNull(i) ?: posted[key] ?: ""
          out += Fx.Post(note(key, silent = inSession))
          shown += key
        }
        out += Fx.Seen(msg.alertId, shown)
      }
      is Msg.Ring -> rings.arrive(msg, now)
      is Msg.RingOver -> rings.over(msg.ringId).forEach { out += Fx.Ringing(it) }
      is Msg.Heartbeat, is Msg.Other -> {}
    }
    msg.cursor?.let { cursor = it }
    if (msg is Msg.Snapshot) epoch = msg.cursor.substringBefore('.')
    rings.tick(now, inSession).forEach { out += Fx.Ringing(it) }
    return out
  }

  /** Time passing: rings to raise or stop. */
  fun tick(now: Long): List<Fx> = rings.tick(now, inSession).map { Fx.Ringing(it) }

  /** An action from this phone left for the desk: the notification loses its buttons. */
  fun sendingNow(key: String): Fx? {
    sending += key
    errors.remove(key)
    return if (key in posted) Fx.Post(note(key, silent = true)) else null
  }

  /**
   * The desk answered the action. Taken: nothing changes here; the stream says what came of it
   * (the item acknowledged, the question answered) and that takes the notification down, so it
   * is never cleared on the phone's own guess. Refused: the buttons come back with why.
   */
  fun answered(key: String, error: String?): Fx? {
    if (error == null) return null
    sending -= key
    errors[key] = error
    return if (key in posted && stillWanted(key)) Fx.Post(note(key, silent = true)) else null
  }

  /** Taken, but the stream never said so (it dropped): the buttons come back. */
  fun stillSending(key: String): Fx? {
    if (!sending.remove(key)) return null
    return if (key in posted && stillWanted(key)) Fx.Post(note(key, silent = true)) else null
  }

  /** Unpaired, or paired with another desk: nothing of this one carries over. */
  fun reset(): List<Fx> {
    val out = posted.keys.map { Fx.Clear(it) } + rings.ringing().map { Fx.Ringing(RingFx.Dismiss(it.ringId)) }
    epoch = null
    cursor = null
    items.clear()
    questions.clear()
    posted.clear()
    sending.clear()
    errors.clear()
    rings.clear()
    return out
  }

  /** The operator swiped the notification away: don't bring it back on an update. */
  fun dismissed(key: String) {
    posted.remove(key)
  }

  fun note(key: String, silent: Boolean): Note =
    Notes.of(key, posted[key], items[key], questions[key], key in sending, errors[key], silent || inSession)

  /** Open, and not acknowledged or snoozed: worth a notification. */
  private fun stillWanted(key: String): Boolean {
    val q = questions[key] ?: items[key]?.question
    if (q != null && !q.open) return false
    val item = items[key] ?: return q != null
    return item.state != "acknowledged" && item.state != "snoozed"
  }

  private fun clear(key: String): Fx {
    posted.remove(key)
    sending.remove(key)
    errors.remove(key)
    return Fx.Clear(key)
  }

  /** The open questions and the items, as the app's Home reads them (raw views, untouched). */
  fun view(): JSONObject = JSONObject()
    .put("items", JSONArray().also { a -> items.values.forEach { a.put(it.raw) } })
    .put("questions", JSONArray().also { a -> questions.values.forEach { a.put(it.raw) } })
    .put("sending", JSONArray(sending.toList()))
    .put("errors", JSONObject(errors as Map<*, *>))

  /** What is kept across a restart of the app's process: the list, the cursor, what is up. */
  fun save(): JSONObject = view()
    .put("epoch", epoch ?: JSONObject.NULL)
    .put("cursor", cursor ?: JSONObject.NULL)
    .put("posted", JSONObject(posted as Map<*, *>))

  fun restore(o: JSONObject) {
    epoch = o.optStringOrNull("epoch")
    cursor = o.optStringOrNull("cursor")
    items.clear()
    o.optJSONArray("items").objects().mapNotNull(Item::from).forEach { items[it.key] = it }
    questions.clear()
    o.optJSONArray("questions").objects().mapNotNull(Question::from).forEach { questions[it.key] = it }
    posted.clear()
    o.optJSONObject("posted")?.let { p -> p.keys().forEach { k -> posted[k] = p.optString(k) } }
  }

  /** The path to open the stream at: resume from the cursor when there is one. */
  fun streamPath(): String = cursor?.let { "/stream?cursor=$it" } ?: "/stream"
}
