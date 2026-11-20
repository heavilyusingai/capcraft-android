package io.github.artcraftandroid.capcraft.engine

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.annotation.RequiresApi
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import io.github.artcraftandroid.capcraft.R
import io.github.artcraftandroid.capcraft.core.cancelExport

/**
 * Keeps the app alive while a video exports with the app in the background, as a foreground
 * service whose notification shows how far the export has got. It does no work of its own: the
 * engine exports on its own thread, and this is started with the export and stopped as it ends.
 *
 * On Android 15+ (API 35) the service uses the `mediaProcessing` foreground service type, which
 * is purpose-built for time-consuming media work such as transcoding. On Android 14 and below it
 * falls back to `dataSync`. The system allows mediaProcessing services a total of 6 hours in any
 * 24-hour period; [`Service.onTimeout`] is called when the limit is reached and the service must
 * stop itself within a few seconds or the system raises a RemoteServiceException.
 *
 * The service is only ever started from direct user interaction (tapping Export), so it has the
 * full six hours after the app goes to the background.
 */
class ExportService : Service() {

    /** How long, in millis, the system allows mediaProcessing services to run in 24 hours: 6 h. */
    private const val MEDIA_PROCESSING_BUDGET_MILLIS = 6L * 60 * 60 * 1000

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // The notification's own Cancel: an export that is running with the app in the background
        // should not need the app reopened to be stopped.
        if (intent?.action == ACTION_CANCEL) {
            runCatching { cancelExport() }
            // The service is kept up by the export's own start and stopped by its finish; only say
            // what is happening while it stops.
            runCatching {
                channel(this)
                val stopping = NotificationCompat.Builder(this, CHANNEL)
                    .setSmallIcon(android.R.drawable.stat_sys_upload)
                    .setContentTitle(getString(R.string.exporting))
                    .setContentText(getString(R.string.export_cancelling))
                    .setOngoing(true)
                    .setOnlyAlertOnce(true)
                    .build()
                NotificationManagerCompat.from(this).notify(ID, stopping)
            }
            // A Cancel that lands after the export has already ended would otherwise leave a service
            // running with nothing to do.
            if (!exporting) stopSelf()
            return START_NOT_STICKY
        }
        val notification = running(this, 0f)
        // Android 15+: mediaProcessing; older: dataSync. startForeground's type arg needs API 29.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S_V_2) {
            startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROCESSING)
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(ID, notification)
        }
        return START_NOT_STICKY
    }

    /**
     * Android 15+ callback: the service's mediaProcessing time budget (6 h / 24 h) has been
     * reached. The service has a few seconds to call stopSelf(); otherwise the system throws a
     * RemoteServiceException. No-op on Android 14 and below, where onTimeout doesn't exist.
     */
    @RequiresApi(Build.VERSION_CODES.S_V_2)
    override fun onTimeout(type: Int, startId: Int) {
        if (type == ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROCESSING) {
            // Stop the export cleanly: the engine thread sees the cancel handle and finishes
            // the current frame, then the service's finish() posts a result notification.
            stopSelf()
        }
    }

        companion object {
        private const val ID = 41
        private const val DONE_ID = 42
        private const val CHANNEL = "export"
        private const val PREFS = "export_state"
        private const val KEY_OUT = "export_out_path"
        private const val KEY_PROGRESS = "export_progress"

        /** Whether an export is running, so a late Cancel does not leave the service with nothing to do. */
        @Volatile private var exporting = false

        /** The action the notification's Cancel sends back to this service. */
        private const val ACTION_CANCEL = "io.github.artcraftandroid.capcraft.CANCEL_EXPORT"

        /** The last share shown, so the notification is redrawn every few percent rather than every frame. */
        @Volatile private var shown = -1

        /** Persists the in-progress export so it can be recovered if the process is killed. */
        private fun saveExportState(context: Context, out: String, progress: Float) {
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
                .putString(KEY_OUT, out)
                .putFloat(KEY_PROGRESS, progress)
                .apply()
        }

        /** Clears the persisted export state once the export has finished or failed. */
        fun clearExportState(context: Context) {
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
                .remove(KEY_OUT)
                .remove(KEY_PROGRESS)
                .apply()
        }

        /** Returns the persisted output path if an export was interrupted, or null. */
        fun incompleteExport(context: Context): String? {
            return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY_OUT, null)
        }

        private fun channel(context: Context) {
            val manager = context.getSystemService(NotificationManager::class.java) ?: return
            if (manager.getNotificationChannel(CHANNEL) == null) {
                manager.createNotificationChannel(NotificationChannel(CHANNEL, context.getString(R.string.export_channel), NotificationManager.IMPORTANCE_LOW))
            }
        }

        /** Opens the app where it was, from a notification. */
        private fun opening(context: Context): PendingIntent? {
            val launch = context.packageManager.getLaunchIntentForPackage(context.packageName) ?: return null
            launch.flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_RESET_TASK_IF_NEEDED
            return PendingIntent.getActivity(context, 0, launch, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }

        /** The PendingIntent that lands back on this service's own Cancel. */
        private fun cancelling(context: Context): PendingIntent {
            val intent = Intent(context, ExportService::class.java).setAction(ACTION_CANCEL)
            return PendingIntent.getService(context, 1, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }

        private fun running(context: Context, fraction: Float) = NotificationCompat.Builder(context, CHANNEL.also { channel(context) })
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentTitle(context.getString(R.string.exporting))
            .setContentText(context.getString(R.string.export_percent, (fraction * 100).toInt()))
            .setProgress(100, (fraction * 100).toInt(), false)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(opening(context))
            .addAction(0, context.getString(R.string.export_cancel), cancelling(context))
            .build()

        /**
         * Starts keeping the app alive for an export, and says whether it did: an export already being
         * kept alive is the one that owns the service, and a second attempt must not stop it.
         */
        fun start(context: Context, out: String): Boolean {
            val owner = !exporting
            exporting = true
            shown = -1
            saveExportState(context, out, 0f)
            runCatching { ContextCompat.startForegroundService(context, Intent(context, ExportService::class.java)) }
            return owner
        }

        /** Shows how far the export has got, every few percent. */
        fun progress(context: Context, fraction: Float) {
            val percent = (fraction * 100).toInt()
            if (percent < shown + 2 && percent < 100) return
            shown = percent
            runCatching { NotificationManagerCompat.from(context).notify(ID, running(context, fraction)) }
        }

        /** Stops keeping the app alive; with [message], leaves a notification saying how it ended. */
        fun finish(context: Context, message: String?) {
            exporting = false
            runCatching { context.stopService(Intent(context, ExportService::class.java)) }
            clearExportState(context)
            if (message == null) return
            runCatching {
                channel(context)
                val done = NotificationCompat.Builder(context, CHANNEL)
                    .setSmallIcon(android.R.drawable.stat_sys_upload_done)
                    .setContentTitle(message)
                    .setAutoCancel(true)
                    .setContentIntent(opening(context))
                    .build()
                NotificationManagerCompat.from(context).notify(DONE_ID, done)
            }
        }
    }
}
