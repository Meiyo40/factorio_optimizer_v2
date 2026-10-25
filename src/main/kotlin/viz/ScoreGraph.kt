package viz

import org.jetbrains.letsPlot.export.ggsave
import org.jetbrains.letsPlot.geom.geomLine
import org.jetbrains.letsPlot.geom.geomPoint
import org.jetbrains.letsPlot.label.ggtitle
import org.jetbrains.letsPlot.label.labs
import org.jetbrains.letsPlot.letsPlot
import org.jetbrains.letsPlot.themes.elementText
import org.jetbrains.letsPlot.themes.theme
import org.jetbrains.letsPlot.themes.themeMinimal

data class timestampToScore(val timestamp: Long, val score: Double)

// The graph is optional; report a failure once per run instead of on every improvement.
private var exportFailureReported = false


fun generateScoreGraph(
    timestampToScore: List<timestampToScore>,
    path: String = ".",
    outputFileName: String = "score_vs_time.svg",
    yLabel: String = "Distance to travel (meters)",
) {
    // 1. FIX: Force Headless mode to prevent libc++abi/AWT crashes
    System.setProperty("java.awt.headless", "true")

    if (timestampToScore.isEmpty()) {
        println("Data list is empty.")
        return
    }

    // 2. Normalize Data
    val startTime = timestampToScore.minOf { it.timestamp }
    val secondsElapsed = timestampToScore.map { (it.timestamp - startTime) / 1000.0 }
    val scores = timestampToScore.map { it.score }

    val plotData = mapOf("seconds" to secondsElapsed, "score" to scores)

    // 3. Build Plot
    val plot = letsPlot(plotData) +
            geomLine(color = "#1f77b4", size = 1.0, alpha = 0.7) { x = "seconds"; y = "score" } +
            geomPoint(color = "#1f77b4", size = 3.0) { x = "seconds"; y = "score" } +
            labs(x = "Time Elapsed (seconds)", y = yLabel) +
            ggtitle("Score over Time") +
            themeMinimal() +
            theme(
                plotTitle = elementText(size = 15),
                axisTitle = elementText(size = 12),
                axisText = elementText(size = 12)
            )

    // 4. Save with Fallback
    try {
        // Try saving as PNG (requires lets-plot-image-export jar)
        ggsave(plot, outputFileName, path = path)
    } catch (e: Throwable) {
        if (!exportFailureReported) {
            exportFailureReported = true
            println("Score graph not written (the layouts are unaffected): $e")
        }
    }
}