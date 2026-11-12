package dev.heka.ghost.call.link

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Log
import org.json.JSONArray
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** The paired desk, as the connection needs it with no JavaScript running. */
class LinkConfig(val deviceId: String, val secret: ByteArray, val addresses: List<String>, val workstation: String)

/**
 * What the connection keeps on the phone: the pairing (device id, secret, addresses), the two
 * switches, and the last list with its cursor, so the app shows it at once on open and the
 * stream resumes where it stopped. Everything but the switches is encrypted with an AES key
 * that lives in the Android Keystore and never leaves it. The app's own secure storage keeps
 * the pairing too (`src/phone/pairing.ts`); the app hands it here (`configure`) whenever it
 * starts paired, so the two never disagree for long.
 */
class LinkStore(context: Context) {
  private val prefs = context.applicationContext.getSharedPreferences("ghost.link", Context.MODE_PRIVATE)

  var enabled: Boolean
    get() = prefs.getBoolean(ENABLED, true)
    set(v) = prefs.edit().putBoolean(ENABLED, v).apply()

  var bootStart: Boolean
    get() = prefs.getBoolean(BOOT, false)
    set(v) = prefs.edit().putBoolean(BOOT, v).apply()

  fun config(): LinkConfig? {
    val device = prefs.getString(DEVICE, null) ?: return null
    val secret = open(prefs.getString(SECRET, null))?.let(Sign::fromB64url) ?: return null
    val addresses = open(prefs.getString(ADDRESSES, null))?.let { text ->
      try {
        JSONArray(text).strings()
      } catch (e: Exception) {
        null
      }
    } ?: return null
    if (addresses.isEmpty() || secret.isEmpty()) return null
    return LinkConfig(device, secret, addresses, prefs.getString(WORKSTATION, "") ?: "")
  }

  /** True when this changed what the connection uses. */
  fun configure(deviceId: String, secretB64: String, addresses: List<String>, workstation: String): Boolean {
    val now = config()
    val same = now != null && now.deviceId == deviceId && Sign.b64url(now.secret) == secretB64.trimEnd('=') && now.addresses == addresses
    if (same && now?.workstation == workstation) return false
    val secret = seal(secretB64) ?: return false
    val list = seal(JSONArray(addresses).toString()) ?: return false
    val e = prefs.edit()
      .putString(DEVICE, deviceId)
      .putString(SECRET, secret)
      .putString(ADDRESSES, list)
      .putString(WORKSTATION, workstation)
    if (now?.deviceId != deviceId) e.remove(STATE) // another pairing: nothing of the last one carries over
    e.apply()
    return !same
  }

  fun forget() {
    prefs.edit().remove(DEVICE).remove(SECRET).remove(ADDRESSES).remove(WORKSTATION).remove(STATE).apply()
  }

  fun saveState(json: String) {
    seal(json)?.let { prefs.edit().putString(STATE, it).apply() }
  }

  fun loadState(): String? = open(prefs.getString(STATE, null))

  // ------------------------------------------------------------------ the Keystore key

  private fun key(): SecretKey? = try {
    val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    (ks.getKey(ALIAS, null) as? SecretKey) ?: KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
      init(
        KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
          .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
          .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
          .setKeySize(256)
          .build()
      )
      generateKey()
    }
  } catch (e: Exception) {
    Log.w(TAG, "no keystore key: ${e.javaClass.simpleName}")
    null
  }

  private fun seal(text: String): String? {
    val k = key() ?: return null
    return try {
      val c = Cipher.getInstance("AES/GCM/NoPadding")
      c.init(Cipher.ENCRYPT_MODE, k)
      Sign.b64url(c.iv) + "." + Sign.b64url(c.doFinal(text.toByteArray(Charsets.UTF_8)))
    } catch (e: Exception) {
      Log.w(TAG, "could not seal: ${e.javaClass.simpleName}")
      null
    }
  }

  /** Null when absent, or when it can't be opened (a backup restored onto another phone). */
  private fun open(sealed: String?): String? {
    if (sealed == null) return null
    return try {
      val (iv, body) = sealed.split('.', limit = 2).map { Sign.fromB64url(it) ?: return null }
      val c = Cipher.getInstance("AES/GCM/NoPadding")
      c.init(Cipher.DECRYPT_MODE, key() ?: return null, GCMParameterSpec(128, iv))
      String(c.doFinal(body), Charsets.UTF_8)
    } catch (e: Exception) {
      Log.w(TAG, "could not open: ${e.javaClass.simpleName}")
      null
    }
  }

  companion object {
    private const val TAG = "GhostLink"
    private const val ALIAS = "ghost.link.v1"
    private const val ENABLED = "enabled"
    private const val BOOT = "boot"
    private const val DEVICE = "device"
    private const val SECRET = "secret"
    private const val ADDRESSES = "addresses"
    private const val WORKSTATION = "workstation"
    private const val STATE = "state"
  }
}
