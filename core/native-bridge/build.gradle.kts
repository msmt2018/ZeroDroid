import org.gradle.api.tasks.Exec

plugins { alias(libs.plugins.android.library) }

android {
    namespace = "dev.zerodroid.core.nativebridge"
    compileSdk = 37
    ndkVersion = "30.0.16248370"

    defaultConfig {
        minSdk = 28
        ndk { abiFilters += "arm64-v8a" }
        externalNativeBuild {
            cmake { cppFlags += listOf("-std=c++20", "-fvisibility=hidden") }
        }
    }

    externalNativeBuild {
        cmake { path = file("../../native/CMakeLists.txt") }
    }
}

val rustAndroidTarget = "aarch64-linux-android"
val rustStaticLibrary = rootProject.file("target/$rustAndroidTarget/release/libzerodroid_ffi.a")
val hostOs = System.getProperty("os.name").lowercase()
val hostArch = System.getProperty("os.arch").lowercase()
val ndkHostTag = when {
    hostOs.contains("mac") && (hostArch == "aarch64" || hostArch == "arm64") -> "darwin-arm64"
    hostOs.contains("mac") -> "darwin-x86_64"
    hostOs.contains("win") -> "windows-x86_64"
    else -> "linux-x86_64"
}
val ndkToolchainBin = File(android.ndkDirectory, "toolchains/llvm/prebuilt/$ndkHostTag/bin")
val rustLinker = File(ndkToolchainBin, "aarch64-linux-android28-clang")

val buildRustAndroidCore by tasks.registering(Exec::class) {
    group = "build"
    description = "Builds the ZeroDroid Rust FFI static library for Android ARM64."
    workingDir(rootProject.projectDir)
    inputs.file(rootProject.file("Cargo.toml"))
    inputs.file(rootProject.file("Cargo.lock"))
    inputs.dir(rootProject.file("rust"))
    outputs.file(rustStaticLibrary)
    doFirst {
        check(rustLinker.isFile) {
            "Android NDK ARM64 Rust linker not found: $rustLinker"
        }
        exec {
            commandLine("rustup", "target", "add", rustAndroidTarget)
        }
    }
    environment("CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER", rustLinker.absolutePath)
    commandLine(
        "cargo", "build", "--locked",
        "--manifest-path", rootProject.file("Cargo.toml").absolutePath,
        "--package", "zerodroid-ffi",
        "--target", rustAndroidTarget,
        "--release"
    )
}

tasks.configureEach {
    if (name.startsWith("configureCMake") || name.startsWith("buildCMake")) {
        dependsOn(buildRustAndroidCore)
    }
}

dependencies { api(project(":core:common")) }
