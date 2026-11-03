package app.clipferry.android

import android.app.Application

class ClipFerryApp : Application() {
    override fun onCreate() {
        super.onCreate()
        CoreController.init(this)
    }
}
