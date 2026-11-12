package dev.heka.ghost.call.link

/** The notification channels, by urgency. The operator can change each in Android's settings. */
enum class Channel(val id: String) {
  /** A question or a change waiting for a yes, at `notify` or `call` urgency: heads-up. */
  ASK("ghost-ask"),
  /** Anything else on the waiting list: an ordinary notification. */
  WAITING("ghost-waiting"),
  /** A question at `digest` urgency: in the shade, no sound. */
  QUIET("ghost-quiet"),
}

/** One button on a notification. */
sealed class NoteAction(val label: String) {
  object Ack : NoteAction("Got it")
  data class Snooze(val minutes: Int) : NoteAction("Later")
  data class Choice(val text: String) : NoteAction(text)
  /** The operator's own words, typed in the notification. */
  object Reply : NoteAction("Answer")
  object Decline : NoteAction("Not answering")
  data class Yes(val changeId: String) : NoteAction("Yes")
  data class No(val changeId: String) : NoteAction("No")
}

/** A notification, as plain data: the Android side only draws it. */
data class Note(
  /** The waiting-list key; also the notification's tag. */
  val key: String,
  val channel: Channel,
  /** Who it is from, always: "Ledger asks", "Watch ci". */
  val title: String,
  /** The asker's words or the item's title, as text. */
  val text: String,
  /** A question's detail, shown when the notification is expanded. */
  val detail: String?,
  val actions: List<NoteAction>,
  /** Choices that didn't fit as buttons: "2 more choices in Ghost". */
  val more: Int,
  /** An action from this phone is on its way; the buttons wait for the desk's word. */
  val sending: Boolean,
  /** What the desk said no with, shown until the item changes. */
  val error: String?,
  /** Posted without sound: during a session on this phone, or an update to one already shown. */
  val silent: Boolean,
)

object Notes {
  /** Android shows at most three actions on a notification; an inline reply is one of them. */
  const val MAX_ACTIONS = 3
  const val LATER_MINUTES = 120

  /** An agent's slug as its name, as `agentName` in @ghost/ui: `heka-workshop` is "Heka Workshop". */
  fun agentName(slug: String): String {
    if (slug.isEmpty() || slug == "ghost") return "Ghost"
    return slug.split('-').filter { it.isNotEmpty() }.joinToString(" ") { w -> w.replaceFirstChar { it.uppercase() } }
  }

  /** Where an item came from, in plain words (the app's Home says the same). */
  fun from(source: String): String {
    val kind = source.substringBefore(':')
    val rest = if (source.contains(':')) source.substringAfter(':') else ""
    return when (kind) {
      "watch" -> "Watch $rest"
      "hook" -> "Webhook $rest"
      "agent" -> agentName(rest)
      "client", "app" -> rest.ifEmpty { "An app" }
      "gate" -> "A change"
      else -> "Ghost"
    }
  }

  /**
   * The notification for one key: a question with its choices (and, unless only they will do,
   * the operator's own words), a change with Yes and No, anything else with Got it and Later.
   */
  fun of(key: String, alertTitle: String?, item: Item?, question: Question?, sending: Boolean, error: String?, silent: Boolean): Note {
    val q = question ?: item?.question
    if (q != null && q.isChange && q.changeId != null) {
      return Note(
        key = key, channel = Channel.ASK,
        title = "${q.askedBy} wants your yes",
        text = q.question.ifEmpty { item?.title ?: alertTitle ?: "A change is waiting for your yes" },
        detail = q.detail,
        actions = if (sending) emptyList() else listOf(NoteAction.Yes(q.changeId), NoteAction.No(q.changeId)),
        more = 0, sending = sending, error = error, silent = silent,
      )
    }
    if (q != null) {
      val room = if (q.choicesOnly) MAX_ACTIONS else MAX_ACTIONS - 1
      val choices = q.choices.take(room).map { NoteAction.Choice(it) }
      val actions = if (q.choicesOnly) choices else listOf(NoteAction.Reply) + choices
      return Note(
        key = key,
        channel = if (q.urgency == "digest") Channel.QUIET else Channel.ASK,
        title = "${q.askedBy} asks",
        text = q.question.ifEmpty { alertTitle ?: "" },
        detail = q.detail,
        actions = if (sending) emptyList() else actions,
        more = (q.choices.size - room).coerceAtLeast(0),
        sending = sending, error = error, silent = silent,
      )
    }
    return Note(
      key = key, channel = Channel.WAITING,
      title = item?.let { from(it.source) } ?: "Ghost",
      text = item?.title ?: alertTitle ?: "Something is waiting for you",
      detail = null,
      actions = if (sending) emptyList() else listOf(NoteAction.Ack, NoteAction.Snooze(LATER_MINUTES)),
      more = 0, sending = sending, error = error, silent = silent,
    )
  }
}
