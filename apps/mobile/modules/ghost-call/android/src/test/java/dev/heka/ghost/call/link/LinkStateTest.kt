package dev.heka.ghost.call.link

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Messages to notifications and to the list, the cursor, and an action's round trip. */
class LinkStateTest {
  private fun msg(json: String): Msg = Msg.parse(json)!!

  private fun question(key: String, state: String = "open", kind: String = "question", choices: String = "[\"Yes\",\"No\"]", only: Boolean = false, change: String? = null, urgency: String = "notify") =
    """{"key":"$key","kind":"$kind","asked_by":"Ledger","source":"agent:ledger","question":"Send the invoice?","detail":null,
       "choices":$choices,"choices_only":$only,"urgency":"$urgency","asked_at":"2026-11-11T09:00:00Z","expires_at":"2026-11-12T09:00:00Z",
       "state":"$state","answer":null,"choice":null,"answered_via":null,"answered_at":null,"change_id":${change?.let { "\"$it\"" } ?: "null"}}"""

  private fun item(key: String, state: String = "new", title: String = "CI failed on main", q: String? = null) =
    """{"key":"$key","source":"watch:ci","title":"$title","state":"$state","first_seen":"2026-11-11T08:00:00Z",
       "changed_at":"2026-11-11T08:00:00Z","age_secs":60${q?.let { ",\"question\":$it" } ?: ""}}"""

  private fun snapshot(cursor: String, items: List<String>, questions: List<String> = emptyList()) =
    msg("""{"type":"snapshot","cursor":"$cursor","items":[${items.joinToString(",")}],"questions":[${questions.joinToString(",")}]}""")

  private fun posts(fx: List<Fx>) = fx.filterIsInstance<Fx.Post>().map { it.note }
  private fun clears(fx: List<Fx>) = fx.filterIsInstance<Fx.Clear>().map { it.key }

  @Test
  fun a_snapshot_replaces_the_list_and_sets_the_cursor() {
    val s = LinkState()
    assertEquals("/stream", s.streamPath())
    s.apply(msg("""{"type":"hello","epoch":"e1","resumed":false,"heartbeat_secs":15}"""), 0)
    s.apply(snapshot("e1.41", listOf(item("watch:ci:1"), item("question:a", q = question("question:a"))), listOf(question("question:a"))), 0)
    assertEquals(listOf("watch:ci:1", "question:a"), s.items.keys.toList())
    assertEquals("e1.41", s.cursor)
    assertEquals("/stream?cursor=e1.41", s.streamPath())
    assertEquals("e1", s.epoch)
  }

  @Test
  fun numbered_messages_and_heartbeats_move_the_cursor() {
    val s = LinkState()
    s.apply(snapshot("e1.3", emptyList()), 0)
    s.apply(msg("""{"type":"item","change":"added","key":"k","item":${item("k")},"seq":4,"cursor":"e1.4"}"""), 0)
    assertEquals("e1.4", s.cursor)
    s.apply(msg("""{"type":"heartbeat","cursor":"e1.4","at":"2026-11-11T09:00:00Z"}"""), 0)
    assertEquals("e1.4", s.cursor)
  }

  @Test
  fun a_desk_that_restarted_drops_the_cursor_until_its_snapshot() {
    val s = LinkState()
    s.apply(msg("""{"type":"hello","epoch":"e1","resumed":false,"heartbeat_secs":15}"""), 0)
    s.apply(snapshot("e1.9", emptyList()), 0)
    s.apply(msg("""{"type":"hello","epoch":"e2","resumed":false,"heartbeat_secs":15}"""), 0)
    assertNull(s.cursor)
    s.apply(snapshot("e2.0", emptyList()), 0)
    assertEquals("e2.0", s.cursor)
  }

  @Test
  fun an_item_added_is_listed_but_not_notified_until_the_desk_alerts() {
    val s = LinkState()
    val fx = s.apply(msg("""{"type":"item","change":"added","key":"k","item":${item("k")},"seq":1,"cursor":"e.1"}"""), 0)
    assertTrue(fx.isEmpty())
    assertTrue("k" in s.items)
  }

  @Test
  fun an_alert_posts_each_item_then_asks_for_seen() {
    val s = LinkState()
    s.apply(msg("""{"type":"item","change":"added","key":"k","item":${item("k")},"seq":1,"cursor":"e.1"}"""), 0)
    val fx = s.apply(msg("""{"type":"alert","alert_id":"a1","keys":["k","unknown"],"titles":["CI failed on main","Ledger asks: x?"],"count":2,"seq":2,"cursor":"e.2"}"""), 0)
    val notes = posts(fx)
    assertEquals(listOf("k", "unknown"), notes.map { it.key })
    assertEquals("Watch ci", notes[0].title)
    assertEquals("CI failed on main", notes[0].text)
    assertEquals(listOf<NoteAction>(NoteAction.Ack, NoteAction.Snooze(120)), notes[0].actions)
    assertEquals("Ledger asks: x?", notes[1].text) // not known yet: the alert's own title
    assertFalse(notes[0].silent)
    val seen = fx.last() as Fx.Seen
    assertEquals("a1", seen.alertId)
    assertEquals(listOf("k", "unknown"), seen.keys)
  }

  @Test
  fun acknowledged_snoozed_or_resolved_anywhere_takes_the_notification_down() {
    val s = LinkState()
    s.apply(snapshot("e.0", listOf(item("a"), item("b"), item("c"))), 0)
    s.apply(msg("""{"type":"alert","alert_id":"x","keys":["a","b","c"],"titles":["a","b","c"],"count":3,"seq":1,"cursor":"e.1"}"""), 0)
    assertEquals(listOf("a"), clears(s.apply(msg("""{"type":"item","change":"changed","key":"a","item":${item("a", "acknowledged")},"seq":2,"cursor":"e.2"}"""), 0)))
    assertEquals(listOf("b"), clears(s.apply(msg("""{"type":"item","change":"changed","key":"b","item":${item("b", "snoozed")},"seq":3,"cursor":"e.3"}"""), 0)))
    assertEquals(listOf("c"), clears(s.apply(msg("""{"type":"item","change":"resolved","key":"c","seq":4,"cursor":"e.4"}"""), 0)))
    assertTrue(s.posted.isEmpty())
    assertFalse("c" in s.items)
  }

  @Test
  fun a_delivered_item_or_a_new_title_updates_the_notification_quietly() {
    val s = LinkState()
    s.apply(snapshot("e.0", listOf(item("a"))), 0)
    s.apply(msg("""{"type":"alert","alert_id":"x","keys":["a"],"titles":["a"],"count":1,"seq":1,"cursor":"e.1"}"""), 0)
    val fx = s.apply(msg("""{"type":"item","change":"changed","key":"a","item":${item("a", "delivered", "CI failed twice")},"seq":2,"cursor":"e.2"}"""), 0)
    val note = posts(fx).single()
    assertEquals("CI failed twice", note.text)
    assertTrue(note.silent)
  }

  @Test
  fun a_question_settled_on_any_surface_clears_its_notification() {
    val s = LinkState()
    s.apply(snapshot("e.0", listOf(item("question:a", q = question("question:a"))), listOf(question("question:a"))), 0)
    s.apply(msg("""{"type":"alert","alert_id":"x","keys":["question:a"],"titles":["Ledger asks"],"count":1,"seq":1,"cursor":"e.1"}"""), 0)
    val fx = s.apply(msg("""{"type":"question","question":${question("question:a", state = "answered")},"seq":2,"cursor":"e.2"}"""), 0)
    assertEquals(listOf("question:a"), clears(fx))
    assertEquals("answered", s.questions["question:a"]!!.state)
  }

  @Test
  fun a_snapshot_after_a_reconnect_clears_what_settled_while_away() {
    val s = LinkState()
    s.apply(snapshot("e.0", listOf(item("a"), item("b"))), 0)
    s.apply(msg("""{"type":"alert","alert_id":"x","keys":["a","b"],"titles":["a","b"],"count":2,"seq":1,"cursor":"e.1"}"""), 0)
    val fx = s.apply(snapshot("f.0", listOf(item("b"))), 0)
    assertEquals(listOf("a"), clears(fx))
    assertEquals(listOf("b"), posts(fx).map { it.key })
  }

  @Test
  fun an_action_keeps_the_notification_until_the_stream_says_what_came_of_it() {
    val s = LinkState()
    s.apply(snapshot("e.0", listOf(item("question:a", q = question("question:a"))), listOf(question("question:a"))), 0)
    s.apply(msg("""{"type":"alert","alert_id":"x","keys":["question:a"],"titles":["t"],"count":1,"seq":1,"cursor":"e.1"}"""), 0)
    val sending = (s.sendingNow("question:a") as Fx.Post).note
    assertTrue(sending.sending)
    assertTrue(sending.actions.isEmpty())
    // Taken: nothing changes on the phone's own guess.
    assertNull(s.answered("question:a", null))
    assertTrue("question:a" in s.posted)
    // The stream's word takes it down.
    val fx = s.apply(msg("""{"type":"question","question":${question("question:a", state = "answered")},"seq":2,"cursor":"e.2"}"""), 0)
    assertEquals(listOf("question:a"), clears(fx))
  }

  @Test
  fun a_refused_action_brings_the_buttons_back_with_why() {
    val s = LinkState()
    s.apply(snapshot("e.0", listOf(item("question:a", q = question("question:a", only = true))), listOf(question("question:a", only = true))), 0)
    s.apply(msg("""{"type":"alert","alert_id":"x","keys":["question:a"],"titles":["t"],"count":1,"seq":1,"cursor":"e.1"}"""), 0)
    s.sendingNow("question:a")
    val note = (s.answered("question:a", "That answer doesn’t fit the question.") as Fx.Post).note
    assertFalse(note.sending)
    assertEquals("That answer doesn’t fit the question.", note.error)
    assertEquals(2, note.actions.size)
  }

  @Test
  fun a_stream_that_never_said_brings_the_buttons_back() {
    val s = LinkState()
    s.apply(snapshot("e.0", listOf(item("a"))), 0)
    s.apply(msg("""{"type":"alert","alert_id":"x","keys":["a"],"titles":["a"],"count":1,"seq":1,"cursor":"e.1"}"""), 0)
    s.sendingNow("a")
    val note = (s.stillSending("a") as Fx.Post).note
    assertFalse(note.sending)
    assertEquals(2, note.actions.size)
    assertNull(s.stillSending("a"))
  }

  @Test
  fun in_a_session_alerts_are_posted_silently() {
    val s = LinkState()
    s.inSession = true
    val fx = s.apply(msg("""{"type":"alert","alert_id":"x","keys":["a"],"titles":["a"],"count":1,"seq":1,"cursor":"e.1"}"""), 0)
    assertTrue(posts(fx).single().silent)
  }

  @Test
  fun the_list_and_the_cursor_survive_a_restart() {
    val s = LinkState()
    s.apply(snapshot("e.5", listOf(item("a")), listOf(question("question:q"))), 0)
    s.apply(msg("""{"type":"alert","alert_id":"x","keys":["a"],"titles":["A title"],"count":1,"seq":6,"cursor":"e.6"}"""), 0)
    val back = LinkState()
    back.restore(JSONObject(s.save().toString()))
    assertEquals("e.6", back.cursor)
    assertEquals("e", back.epoch)
    assertEquals(listOf("a"), back.items.keys.toList())
    assertEquals(listOf("question:q"), back.questions.keys.toList())
    assertEquals("A title", back.posted["a"])
    assertEquals("/stream?cursor=e.6", back.streamPath())
  }

  @Test
  fun forgetting_the_desk_takes_everything_down() {
    val s = LinkState()
    s.apply(snapshot("e.0", listOf(item("a"))), 0)
    s.apply(msg("""{"type":"alert","alert_id":"x","keys":["a"],"titles":["a"],"count":1,"seq":1,"cursor":"e.1"}"""), 0)
    assertEquals(listOf("a"), clears(s.reset()))
    assertNull(s.cursor)
    assertTrue(s.items.isEmpty())
  }

  @Test
  fun a_dismissed_notification_is_not_brought_back_by_an_update() {
    val s = LinkState()
    s.apply(snapshot("e.0", listOf(item("a"))), 0)
    s.apply(msg("""{"type":"alert","alert_id":"x","keys":["a"],"titles":["a"],"count":1,"seq":1,"cursor":"e.1"}"""), 0)
    s.dismissed("a")
    val fx = s.apply(msg("""{"type":"item","change":"changed","key":"a","item":${item("a", "delivered", "new title")},"seq":2,"cursor":"e.2"}"""), 0)
    assertTrue(fx.isEmpty())
  }

  @Test
  fun an_unknown_message_type_is_kept_for_its_cursor_only() {
    val s = LinkState()
    val fx = s.apply(msg("""{"type":"something_new","seq":7,"cursor":"e.7"}"""), 0)
    assertTrue(fx.isEmpty())
    assertEquals("e.7", s.cursor)
  }
}
