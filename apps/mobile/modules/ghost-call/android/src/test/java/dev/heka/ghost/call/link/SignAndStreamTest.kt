package dev.heka.ghost.call.link

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/** The phone's signing and stream reading against the daemon's own vectors. */
class SignAndStreamTest {
  private val v = JSONObject(File(System.getProperty("ghost.vectors") ?: "../../../../../packages/api/phone-vectors.json").readText())
  private val secret = Sign.fromB64url(v.getString("secret"))!!
  private val nonce = v.getString("nonce")
  private val time = v.getString("time")

  @Test
  fun requests_are_signed_as_the_daemon_signs_them() {
    val reqs = v.getJSONArray("requests")
    for (i in 0 until reqs.length()) {
      val r = reqs.getJSONObject(i)
      val body = r.getString("body").toByteArray(Charsets.UTF_8)
      assertEquals(r.getString("string"), Sign.requestString(r.getString("method"), r.getString("path"), time, nonce, body))
      val h = Sign.headers(v.getString("device"), secret, r.getString("method"), r.getString("path"), body, time.toLong(), nonce)
      assertEquals(r.getString("signature"), h["X-Ghost-Signature"])
    }
  }

  @Test
  fun responses_verify_and_a_changed_one_does_not() {
    val rs = v.getJSONArray("responses")
    for (i in 0 until rs.length()) {
      val r = rs.getJSONObject(i)
      val body = r.getString("body").toByteArray(Charsets.UTF_8)
      assertTrue(Sign.verifyResponse(secret, r.getInt("status"), nonce, time, r.getString("signature"), body))
      assertFalse(Sign.verifyResponse(secret, r.getInt("status") + 1, nonce, time, r.getString("signature"), body))
      assertFalse(Sign.verifyResponse(secret, r.getInt("status"), nonce, time, null, body))
    }
  }

  @Test
  fun base64url_round_trips() {
    assertEquals(v.getString("secret"), Sign.b64url(secret))
    assertEquals(32, secret.size)
    assertNull(Sign.fromB64url("not base64!"))
  }

  @Test
  fun every_frame_parses_and_verifies_over_its_exact_bytes() {
    val events = v.getJSONArray("events")
    val parser = SseParser()
    val got = mutableListOf<Msg>()
    for (i in 0 until events.length()) {
      val e = events.getJSONObject(i)
      assertEquals(e.getString("string"), Sign.eventString(nonce, e.getString("msg").toByteArray(Charsets.UTF_8)))
      for (line in e.getString("frame").split("\n")) {
        val ev = parser.line(line) ?: continue
        val d = decode(ev.data, secret, nonce)
        assertTrue("frame $i decodes", d is Decoded.Ok)
        got += (d as Decoded.Ok).msg
      }
    }
    assertEquals(3, got.size)
    val hello = got[0] as Msg.Hello
    assertEquals("3fa9c2d1", hello.epoch)
    assertFalse(hello.resumed)
    val q = (got[1] as Msg.QuestionChange)
    assertEquals("3fa9c2d1.42", q.cursor)
    assertEquals("Should the “October” invoice go out today?\nIt is €1,200.", q.question.question)
    assertEquals(listOf("Yes", "No"), q.question.choices)
    assertEquals("Ledger", q.question.askedBy)
    assertTrue(got[2] is Msg.Heartbeat)
  }

  @Test
  fun a_message_from_another_stream_or_changed_on_the_way_is_dropped() {
    val e = v.getJSONArray("events").getJSONObject(1)
    val data = JSONObject().put("msg", e.getString("msg")).put("sig", e.getString("sig")).toString()
    assertTrue(decode(data, secret, nonce) is Decoded.Ok)
    assertEquals(Decoded.BadSignature, decode(data, secret, "ffeeddccbbaa99887766554433221100"))
    val changed = JSONObject().put("msg", e.getString("msg").replace("Ledger", "Ghost")).put("sig", e.getString("sig")).toString()
    assertEquals(Decoded.BadSignature, decode(changed, secret, nonce))
    assertEquals(Decoded.BadSignature, decode("not json", secret, nonce))
    assertEquals(Decoded.BadSignature, decode("{\"msg\":\"{}\"}", secret, nonce))
  }

  @Test
  fun sse_lines_follow_the_rules() {
    val p = SseParser()
    assertNull(p.line(": a comment"))
    assertNull(p.line("event: item"))
    assertNull(p.line("id: e.1"))
    assertNull(p.line("data: one"))
    assertNull(p.line("data:two\r"))
    val ev = p.line("")!!
    assertEquals("item", ev.event)
    assertEquals("e.1", ev.id)
    assertEquals("one\ntwo", ev.data)
    // A blank line with nothing before it dispatches nothing; fields don't leak into the next.
    assertNull(p.line(""))
    p.line("data: x")
    val next = p.line("")!!
    assertNull(next.event)
    assertNull(next.id)
  }
}
