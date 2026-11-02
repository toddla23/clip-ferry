package app.clipferry.android

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.GeneralSecurityException
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import uniffi.clip_ferry_ffi.FfiException
import uniffi.clip_ferry_ffi.SecretVault

/**
 * 코어의 비밀(기기 개인키, 그룹 키, 로컬 저장 키)을 두는 곳.
 * AES-256-GCM 키는 Android Keystore 안에서만 쓰이고 기기 밖으로 나오지 않는다.
 * 값은 이 키로 봉인해 SharedPreferences에 두며, 이름을 AAD로 묶어 다른 이름 자리로 옮겨 붙인 값은 열리지 않는다.
 * 열리지 않는 값은 "없음"으로 돌려준다. 코어는 한쪽만 남은 상태를 거부하므로(조용히 새로 만들지 않는다) 안전하다.
 */
class KeystoreVault(context: Context) : SecretVault {
    private val prefs = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    private fun secretKey(): SecretKey {
        val store = KeyStore.getInstance(PROVIDER).apply { load(null) }
        (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER)
        generator.init(
            KeyGenParameterSpec.Builder(
                ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build()
        )
        return generator.generateKey()
    }

    override fun get(key: String): ByteArray? {
        val stored = prefs.getString(key, null) ?: return null
        return try {
            val raw = Base64.decode(stored, Base64.NO_WRAP)
            if (raw.size <= IV_LEN) return null
            val cipher = Cipher.getInstance(TRANSFORM)
            cipher.init(
                Cipher.DECRYPT_MODE,
                secretKey(),
                GCMParameterSpec(TAG_BITS, raw, 0, IV_LEN),
            )
            cipher.updateAAD(key.toByteArray(Charsets.UTF_8))
            cipher.doFinal(raw, IV_LEN, raw.size - IV_LEN)
        } catch (_: GeneralSecurityException) {
            null
        } catch (_: IllegalArgumentException) {
            null
        }
    }

    override fun set(key: String, value: ByteArray) {
        try {
            val cipher = Cipher.getInstance(TRANSFORM)
            cipher.init(Cipher.ENCRYPT_MODE, secretKey())
            cipher.updateAAD(key.toByteArray(Charsets.UTF_8))
            val sealed = cipher.iv + cipher.doFinal(value)
            // 쓰는 도중 앱이 죽어도 반쯤 쓴 값이 남지 않도록 동기 저장한다.
            if (!prefs.edit().putString(key, Base64.encodeToString(sealed, Base64.NO_WRAP)).commit()) {
                throw FfiException.Failed("비밀 저장소에 쓰지 못했습니다")
            }
        } catch (e: GeneralSecurityException) {
            throw FfiException.Failed("비밀 저장소 오류")
        }
    }

    override fun delete(key: String) {
        if (!prefs.edit().remove(key).commit()) {
            throw FfiException.Failed("비밀 저장소에서 지우지 못했습니다")
        }
    }

    private companion object {
        const val PREFS = "clip_ferry_vault"
        const val PROVIDER = "AndroidKeyStore"
        const val ALIAS = "clip_ferry_vault_key"
        const val TRANSFORM = "AES/GCM/NoPadding"
        const val IV_LEN = 12
        const val TAG_BITS = 128
    }
}
