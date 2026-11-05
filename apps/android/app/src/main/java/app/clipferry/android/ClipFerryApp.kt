package app.clipferry.android

import android.app.Application

class ClipFerryApp : Application() {
    override fun onCreate() {
        super.onCreate()
        ShareActivity.clearStale(this)
        CoreController.init(this)
        ShareShortcut.publish(this)
    }
}
