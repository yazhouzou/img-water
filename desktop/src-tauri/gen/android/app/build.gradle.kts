import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

// 版本号单一来源 = src-tauri/tauri.conf.json 的 version。tauri.properties 由 CLI 生成且被
// .gitignore 掉：CI 全新检出时该文件不存在（gen/android 已入库 → 不会重跑 android init），
// gradle 会静默退回 versionName=1.0/versionCode=1，导致线上 APK 版本号是假的。
// 这里直接读配置；versionCode 沿用 Tauri 默认公式 major*1000000 + minor*1000 + patch
// （0.3.4 → 3004，与历史一致；0.5.5 → 5005，单调递增，满足 Play 商店要求）。
val appVersion: String? = run {
    val conf = file("../../../tauri.conf.json")
    if (!conf.exists()) null
    else Regex("\"version\"\\s*:\\s*\"([^\"]+)\"").find(conf.readText())?.groupValues?.get(1)
}

fun semverToVersionCode(v: String): Int {
    val parts = v.trim().removePrefix("v").split(".")
    fun at(i: Int) = parts.getOrNull(i)?.takeWhile { it.isDigit() }?.toIntOrNull() ?: 0
    return at(0) * 1000000 + at(1) * 1000 + at(2)
}

android {
    compileSdk = 36
    namespace = "com.yazhouzou.doubaoWatermarkRemover"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "com.yazhouzou.doubaoWatermarkRemover"
        minSdk = 24
        targetSdk = 36
        versionCode = appVersion?.let { semverToVersionCode(it) }
            ?: tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = appVersion ?: tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }
    signingConfigs {
        create("ciRelease") {
            // 公开发布签名接入点：CI 注入以下环境变量后自动启用 release 签名，
            // 未配置时回退 debug 签名（保证始终可安装）。
            val storeFilePath = System.getenv("ANDROID_KEYSTORE_FILE") ?: return@create
            this.storeFile = file(storeFilePath)
            this.storePassword = System.getenv("ANDROID_KEYSTORE_PASSWORD") ?: return@create
            this.keyAlias = System.getenv("ANDROID_KEY_ALIAS") ?: return@create
            this.keyPassword = System.getenv("ANDROID_KEY_PASSWORD") ?: return@create
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            isMinifyEnabled = true
            proguardFiles(
                *fileTree(".") { include("**/*.pro") }
                    .plus(getDefaultProguardFile("proguard-android-optimize.txt"))
                    .toList().toTypedArray()
            )
            signingConfig = if (System.getenv("ANDROID_KEYSTORE_FILE") != null) {
                signingConfigs.getByName("ciRelease")
            } else {
                signingConfigs.getByName("debug")
            }
        }
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
    buildFeatures {
        buildConfig = true
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = "tauri.build.gradle.kts")