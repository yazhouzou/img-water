# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile

# MainActivity 的 exportToGallery/shareFiles/onWebViewCreate 由 Rust 端通过 JNI 按名字
# 反射调用。release 构建开启 R8，若不加规则这些"无 Java 调用点"的方法会被裁掉/改名，
# 导致保存到相册与分享静默失效。
-keep class com.yazhouzou.doubaoWatermarkRemover.MainActivity { *; }