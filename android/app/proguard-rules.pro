# JNA and the generated UniFFI bindings are reached reflectively from native code.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class dev.phonelink.core.** { *; }
-dontwarn java.awt.**
