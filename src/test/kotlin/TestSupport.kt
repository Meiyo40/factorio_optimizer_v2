import config.RunConfiguration
import data.RootData
import data.loadRootData
import data.readResourceOrFile

/** recipes.json parsed once for the whole test run. */
object TestData {
    val data: RootData by lazy { loadRootData() }

    fun job(name: String): RunConfiguration = RunConfiguration.load("jobs/$name.json")

    fun text(name: String): String = readResourceOrFile(name)
}
