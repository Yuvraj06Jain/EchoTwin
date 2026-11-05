// Runs the generated Kotlin bindings on a desktop JVM against the host build of the Rust
// core, with the real phonelinkd daemon as the "laptop". Same Kotlin code the Android app
// uses; only the .so differs.  Run via ../../scripts/test-kotlin-bindings.sh
plugins {
    kotlin("jvm") version "2.0.21"
}

repositories { mavenCentral() }

val repoRoot = rootDir.resolve("../..").canonicalFile
val rustTarget = repoRoot.resolve("target/debug")

sourceSets["test"].kotlin.srcDir(repoRoot.resolve("android/app/src/main/java/dev/phonelink/core"))

dependencies {
    testImplementation("net.java.dev.jna:jna:5.17.0")
    testImplementation(kotlin("test"))
}

kotlin { jvmToolchain(21) }

tasks.test {
    useJUnitPlatform()
    systemProperty("jna.library.path", rustTarget.path)
    environment("PHONELINK_BIN_DIR", rustTarget.path)
    testLogging { events("passed", "failed"); showStandardStreams = true; exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL }
}
