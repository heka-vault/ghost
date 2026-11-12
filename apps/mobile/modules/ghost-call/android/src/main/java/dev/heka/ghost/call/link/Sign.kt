package dev.heka.ghost.call.link

import java.security.MessageDigest
import java.security.SecureRandom
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * The phone protocol's signatures (docs/phone-protocol.md), the same as `src/phone/signing.ts`
 * and the daemon's `ghostd::phone::signing`: HMAC-SHA256 under the paired device's secret,
 * base64url without padding. Pure JVM code, so the shared vectors test it off the phone.
 */
object Sign {
  private const val B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
  private val random = SecureRandom()

  fun b64url(bytes: ByteArray): String {
    val out = StringBuilder((bytes.size * 4 + 2) / 3)
    var i = 0
    while (i < bytes.size) {
      val b0 = bytes[i].toInt() and 0xff
      val b1 = if (i + 1 < bytes.size) bytes[i + 1].toInt() and 0xff else 0
      val b2 = if (i + 2 < bytes.size) bytes[i + 2].toInt() and 0xff else 0
      val n = (b0 shl 16) or (b1 shl 8) or b2
      out.append(B64[(n shr 18) and 63]).append(B64[(n shr 12) and 63])
      if (i + 1 < bytes.size) out.append(B64[(n shr 6) and 63])
      if (i + 2 < bytes.size) out.append(B64[n and 63])
      i += 3
    }
    return out.toString()
  }

  /** base64url, with or without padding; null when it isn't. */
  fun fromB64url(text: String): ByteArray? {
    val out = java.io.ByteArrayOutputStream()
    var acc = 0
    var bits = 0
    for (ch in text.trimEnd('=')) {
      val c = when (ch) { '+' -> '-'; '/' -> '_'; else -> ch }
      val v = B64.indexOf(c)
      if (v < 0) return null
      acc = (acc shl 6) or v
      bits += 6
      if (bits >= 8) {
        bits -= 8
        out.write((acc shr bits) and 0xff)
      }
    }
    return out.toByteArray()
  }

  fun hex(bytes: ByteArray): String = bytes.joinToString("") { "%02x".format(it.toInt() and 0xff) }

  fun sha256Hex(bytes: ByteArray): String = hex(MessageDigest.getInstance("SHA-256").digest(bytes))

  fun mac(secret: ByteArray, text: String): String {
    val m = Mac.getInstance("HmacSHA256")
    m.init(SecretKeySpec(secret, "HmacSHA256"))
    return b64url(m.doFinal(text.toByteArray(Charsets.UTF_8)))
  }

  /** Constant time, any lengths. */
  fun same(a: String, b: String): Boolean =
    MessageDigest.isEqual(a.toByteArray(Charsets.UTF_8), b.toByteArray(Charsets.UTF_8))

  fun nonce(): String = ByteArray(16).also { random.nextBytes(it) }.let(::hex)

  fun requestString(method: String, pathAndQuery: String, time: String, nonce: String, body: ByteArray): String =
    listOf("ghost-v1", method.uppercase(), pathAndQuery, time, nonce, sha256Hex(body)).joinToString("\n")

  fun responseString(status: Int, nonce: String, time: String, body: ByteArray): String =
    listOf("ghost-v1-response", status.toString(), nonce, time, sha256Hex(body)).joinToString("\n")

  fun eventString(nonce: String, msg: ByteArray): String =
    listOf("ghost-v1-event", nonce, sha256Hex(msg)).joinToString("\n")

  /** The four request headers. */
  fun headers(deviceId: String, secret: ByteArray, method: String, pathAndQuery: String, body: ByteArray, nowSecs: Long, nonce: String): Map<String, String> {
    val time = nowSecs.toString()
    return mapOf(
      "X-Ghost-Device" to deviceId,
      "X-Ghost-Time" to time,
      "X-Ghost-Nonce" to nonce,
      "X-Ghost-Signature" to mac(secret, requestString(method, pathAndQuery, time, nonce, body)),
    )
  }

  /** Whether a response came from the paired desk, in answer to the request with this nonce. */
  fun verifyResponse(secret: ByteArray, status: Int, nonce: String, time: String?, signature: String?, body: ByteArray): Boolean {
    if (time.isNullOrEmpty() || signature.isNullOrEmpty()) return false
    return same(signature, mac(secret, responseString(status, nonce, time, body)))
  }

  /** One message on the live stream: signed over the stream request's nonce and its exact bytes. */
  fun verifyEvent(secret: ByteArray, nonce: String, msg: String, signature: String): Boolean =
    same(signature, mac(secret, eventString(nonce, msg.toByteArray(Charsets.UTF_8))))
}
