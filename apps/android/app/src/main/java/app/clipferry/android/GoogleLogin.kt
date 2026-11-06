package app.clipferry.android

import android.app.Activity
import android.content.Context
import androidx.credentials.ClearCredentialStateRequest
import androidx.credentials.CredentialManager
import androidx.credentials.CustomCredential
import androidx.credentials.GetCredentialRequest
import androidx.credentials.exceptions.GetCredentialCancellationException
import androidx.credentials.exceptions.GetCredentialException
import androidx.credentials.exceptions.NoCredentialException
import com.google.android.libraries.identity.googleid.GetSignInWithGoogleOption
import com.google.android.libraries.identity.googleid.GoogleIdTokenCredential
import com.google.android.libraries.identity.googleid.GoogleIdTokenParsingException

/** 사용자에게 그대로 보여 줄 로그인 실패 이유. */
class LoginFailed(message: String) : Exception(message)

/**
 * 구글 로그인 (F35). Credential Manager로 시스템의 구글 계정 선택 창을 띄워 ID 토큰만 받는다.
 * 토큰 검증·계정·갱신은 계정 서버와 코어(`ffi`)가 한다. 이 앱은 구글 비밀값을 갖지 않는다.
 *
 * `serverClientId`는 계정 서버가 알려 주는 "웹 애플리케이션" 클라이언트 ID다. 받은 ID 토큰의 `aud`가 이 값이다.
 * 구글은 이 앱의 패키지 이름과 서명 인증서(SHA-1)로 등록된 "Android" 클라이언트가 같은 프로젝트에
 * 있을 때만 토큰을 준다. 없으면 "Developer console is not set up correctly" 오류가 난다.
 */
object GoogleLogin {
    suspend fun idToken(activity: Activity, serverClientId: String): String {
        val option = GetSignInWithGoogleOption.Builder(serverClientId).build()
        val request = GetCredentialRequest.Builder().addCredentialOption(option).build()
        val result = try {
            CredentialManager.create(activity).getCredential(activity, request)
        } catch (e: GetCredentialCancellationException) {
            throw LoginFailed("로그인을 취소했습니다")
        } catch (e: NoCredentialException) {
            throw LoginFailed("이 기기에서 쓸 수 있는 구글 계정이 없습니다. 기기 설정에서 구글 계정을 추가하세요")
        } catch (e: GetCredentialException) {
            throw LoginFailed(
                "구글 로그인 창을 열지 못했습니다 (${e.message ?: e.type}). " +
                    "구글 콘솔의 Android 클라이언트(패키지 이름·SHA-1)를 확인하세요",
            )
        }
        val credential = result.credential
        if (credential is CustomCredential &&
            credential.type == GoogleIdTokenCredential.TYPE_GOOGLE_ID_TOKEN_CREDENTIAL
        ) {
            try {
                return GoogleIdTokenCredential.createFrom(credential.data).idToken
            } catch (_: GoogleIdTokenParsingException) {
                // 아래로
            }
        }
        throw LoginFailed("구글 로그인 결과를 읽지 못했습니다")
    }

    /** 로그아웃할 때: 다음 로그인에서 계정을 다시 고를 수 있게 시스템의 선택 상태를 지운다. 실패해도 된다. */
    suspend fun clear(context: Context) {
        try {
            CredentialManager.create(context).clearCredentialState(ClearCredentialStateRequest())
        } catch (_: Exception) {
        }
    }
}
