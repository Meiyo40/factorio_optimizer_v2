plugins {
    kotlin("jvm") version "2.4.21"
    kotlin("plugin.serialization") version "2.4.21"
    // Add application plugin so `./gradlew run` is available
    application

}

group = "org.example"
version = "2.0"

repositories {
    mavenCentral()

}

dependencies {
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.12.0")
    implementation("com.google.code.gson:gson:2.15.0")
    implementation("com.google.ortools:ortools-java:9.15.6755")
    implementation("org.jetbrains.lets-plot:lets-plot-kotlin:4.15.0")
    implementation("org.jetbrains.lets-plot:lets-plot-image-export:4.8.2")

    testImplementation(kotlin("test"))
}

kotlin {
    // Java 25 (LTS); the devcontainer uses the same JDK.
    jvmToolchain(25)
}

tasks.test {
    useJUnitPlatform()
    // OR-Tools loads its native library (the LP planner); Java 25 asks for that to be allowed explicitly.
    jvmArgs("--enable-native-access=ALL-UNNAMED")
}

// Configure the application plugin: the Kotlin top-level `main` in `Main.kt` becomes class `MainKt`
application {
    // If your `main` is in a package, use the fully-qualified name, e.g. "com.example.MainKt"
    mainClass.set("MainKt")
    applicationDefaultJvmArgs = listOf("--enable-native-access=ALL-UNNAMED")
}

tasks.named<JavaExec>("run") {
    // If 'ram' is passed from CLI, use it. Otherwise default to 2g (or whatever you had).
    val ram = project.findProperty("ram") ?: "6g"
    jvmArgs = listOf("-Xmx$ram", "--enable-native-access=ALL-UNNAMED")
}