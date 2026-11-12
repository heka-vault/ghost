package dev.heka.ghost.call.link

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.random.Random

/** The ring's lifecycle, what a notification offers, and why an action is safe to send twice. */
class RingsNotesActionsTest {
  private fun ring(id: String, secs: Int = 30) = Msg.Ring(id, "question:a", "ledger", "Ledger asks: x?", "Ledger asks: x? Yes or no?", secs, "e.1")

  // ------------------------------------------------------------------ rings

  @Test
  fun a_ring_rings_after_a_short_settle_and_stops_when_it_is_over() {
    val r = Rings()
    r.arrive(ring("r1"), 1_000)
    assertTrue(r.tick(1_100, false).isEmpty())
    val show = r.tick(1_000 + Rings.SETTLE_MS, false).single() as RingFx.Show
    assertEquals("r1", show.call.ringId)
    assertEquals(30_000 - Rings.SETTLE_MS, show.remainingMs)
    assertEquals(listOf<RingFx>(RingFx.Dismiss("r1")), r.over("r1"))
  }

  @Test
  fun a_replayed_ring_whose_over_follows_in_the_same_burst_never_rings() {
    val r = Rings()
    r.arrive(ring("r1"), 5_000)
    assertTrue(r.over("r1").isEmpty())
    assertTrue(r.tick(10_000, false).isEmpty())
  }

  @Test
  fun a_ring_rings_for_ring_secs_at_most() {
    val r = Rings()
    r.arrive(ring("r1", secs = 30), 0)
    r.tick(Rings.SETTLE_MS, false)
    assertEquals(listOf<RingFx>(RingFx.Dismiss("r1")), r.tick(30_000, false))
    assertTrue(r.ringing().isEmpty())
  }

  @Test
  fun a_ring_older_than_its_time_is_not_shown() {
    val r = Rings()
    r.arrive(ring("r1", secs = 1), 0)
    assertTrue(r.tick(2_000, false).isEmpty())
    assertTrue(r.ringing().isEmpty())
  }

  @Test
  fun a_ring_id_rings_once_even_when_replayed_after_another_reconnect() {
    val r = Rings()
    r.arrive(ring("r1"), 0)
    r.tick(Rings.SETTLE_MS, false)
    r.over("r1") // answered here
    r.arrive(ring("r1"), 60_000)
    assertTrue(r.tick(60_000 + Rings.SETTLE_MS, false).isEmpty())
  }

  @Test
  fun during_a_session_on_this_phone_a_ring_is_not_shown() {
    val r = Rings()
    r.arrive(ring("r1"), 0)
    assertTrue(r.tick(Rings.SETTLE_MS, true).isEmpty())
    assertNull(r.live("r1"))
  }

  @Test
  fun the_next_deadline_is_the_settle_then_the_end() {
    val r = Rings()
    assertNull(r.nextDeadline())
    r.arrive(ring("r1"), 100)
    assertEquals(100 + Rings.SETTLE_MS, r.nextDeadline())
    r.tick(100 + Rings.SETTLE_MS, false)
    assertEquals(30_100L, r.nextDeadline())
  }

  @Test
  fun the_state_rings_and_stops_through_its_effects() {
    val s = LinkState()
    assertTrue(s.apply(Msg.parse("""{"type":"ring","ring_id":"r9","key":"question:a","agent":"ledger","title":"t","question":null,"ring_secs":30,"seq":1,"cursor":"e.1"}""")!!, 0).isEmpty())
    val shown = s.tick(Rings.SETTLE_MS).single() as Fx.Ringing
    assertTrue(shown.fx is RingFx.Show)
    val over = s.apply(Msg.parse("""{"type":"ring_over","ring_id":"r9","why":"timed_out","seq":2,"cursor":"e.2"}""")!!, 1_000)
    assertEquals(listOf<Fx>(Fx.Ringing(RingFx.Dismiss("r9"))), over)
  }

  // ------------------------------------------------------------------ notifications

  private fun q(choices: List<String>, only: Boolean, kind: String = "question", change: String? = null, urgency: String = "notify") =
    Question("question:a", kind, "Ledger", "agent:ledger", "Send it?", "€1,200", choices, only, urgency, "open", change, "2026-11-11T09:00:00Z", JSONObject())

  @Test
  fun a_question_offers_its_choices_and_the_operators_own_words() {
    val n = Notes.of("question:a", null, null, q(listOf("Yes", "No", "Later"), only = false), false, null, false)
    assertEquals("Ledger asks", n.title)
    assertEquals("Send it?", n.text)
    assertEquals(listOf(NoteAction.Reply, NoteAction.Choice("Yes"), NoteAction.Choice("No")), n.actions)
    assertEquals(1, n.more)
    assertEquals(Channel.ASK, n.channel)
  }

  @Test
  fun only_the_choices_when_only_they_will_do_and_at_most_three() {
    val n = Notes.of("question:a", null, null, q(listOf("A", "B", "C", "D"), only = true), false, null, false)
    assertEquals(listOf(NoteAction.Choice("A"), NoteAction.Choice("B"), NoteAction.Choice("C")), n.actions)
    assertEquals(1, n.more)
    assertTrue(n.actions.size <= Notes.MAX_ACTIONS)
  }

  @Test
  fun a_change_is_yes_and_no_never_an_answer() {
    val n = Notes.of("change:p1", null, null, q(listOf("Yes", "No"), only = true, kind = "change", change = "p1"), false, null, false)
    assertEquals(listOf(NoteAction.Yes("p1"), NoteAction.No("p1")), n.actions)
    assertEquals("Ledger wants your yes", n.title)
  }

  @Test
  fun a_digest_question_is_quiet_and_a_sending_one_has_no_buttons() {
    assertEquals(Channel.QUIET, Notes.of("k", null, null, q(emptyList(), false, urgency = "digest"), false, null, false).channel)
    assertTrue(Notes.of("k", null, null, q(listOf("Yes"), false), true, null, false).actions.isEmpty())
  }

  @Test
  fun who_it_is_from_in_plain_words() {
    assertEquals("Watch ci", Notes.from("watch:ci"))
    assertEquals("Webhook github", Notes.from("hook:github"))
    assertEquals("Heka Workshop", Notes.from("agent:heka-workshop"))
    assertEquals("A change", Notes.from("gate"))
    assertEquals("Ghost", Notes.agentName("ghost"))
  }

  // ------------------------------------------------------------------ actions

  @Test
  fun every_action_has_its_own_id_and_a_retry_sends_the_same_bytes() {
    val a = Action.answer("question:a", "Send it today")
    val b = Action.answer("question:a", "Send it today")
    assertNotEquals(a.id, b.id)
    val body = JSONObject(a.body)
    assertEquals(a.id, body.getString("id"))
    assertEquals("question:a", body.getString("key"))
    assertEquals("Send it today", body.getString("text"))
    // The action is a value: a retry sends `a.body` again, the same `id` and the same bytes.
    assertEquals(a.body, a.copy().body)
  }

  @Test
  fun a_change_goes_to_changes_by_its_proposal_id() {
    val yes = Action.confirm("change:p1", "p1", id = "fixed")
    assertEquals("/changes/confirm", yes.path)
    val body = JSONObject(yes.body)
    assertEquals("fixed", body.getString("id"))
    assertEquals("p1", body.getString("change"))
    assertEquals(2, body.length())
    assertEquals("/changes/cancel", Action.cancel("change:p1", "p1").path)
  }

  @Test
  fun only_a_signed_answer_ends_the_retries_and_they_stop_before_the_desk_forgets_the_id() {
    assertEquals(Next.Done(409, null), Retry.next(Tried.Answered(409, null), 3, 1_000))
    assertTrue(Retry.next(Tried.Failed, 0, 0) is Next.Again)
    assertEquals(Next.GiveUp, Retry.next(Tried.Failed, 8, Retry.GIVE_UP_MS))
    assertTrue(Retry.GIVE_UP_MS < 10 * 60 * 1000)
  }

  @Test
  fun refusals_read_as_plain_words() {
    assertNull(Retry.errorText(200, JSONObject().put("ok", true)))
    assertEquals("Already answered.", Retry.errorText(409, JSONObject().put("error", "settled").put("state", "answered")))
    assertEquals("Only Yes or No.", Retry.errorText(422, JSONObject().put("error", "bad_answer").put("detail", "Only Yes or No.")))
  }

  @Test
  fun backoff_doubles_to_thirty_seconds_with_jitter() {
    assertEquals(1_000, Backoff.baseMs(0))
    assertEquals(2_000, Backoff.baseMs(1))
    assertEquals(16_000, Backoff.baseMs(4))
    assertEquals(30_000, Backoff.baseMs(5))
    assertEquals(30_000, Backoff.baseMs(50))
    val rnd = Random(7)
    repeat(200) {
      val d = Backoff.delayMs(5, rnd)
      assertTrue(d in 22_500..37_500)
    }
  }

  @Test
  fun home_first_then_the_tunnel() {
    val list = listOf("192.168.1.20:8765", "10.8.0.4:8765")
    assertEquals("home", viaOf(list, "192.168.1.20:8765"))
    assertEquals("tunnel", viaOf(list, "10.8.0.4:8765"))
    assertNull(viaOf(list, null))
  }
}
